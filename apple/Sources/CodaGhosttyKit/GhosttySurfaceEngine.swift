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
    public let bg: GhosttyRGBColor?
    public let bold: Bool
    public let dim: Bool
    public let inverse: Bool

    public init(
        id: Int,
        text: String,
        fg: GhosttyRGBColor? = nil,
        bg: GhosttyRGBColor? = nil,
        bold: Bool = false,
        dim: Bool = false,
        inverse: Bool = false
    ) {
        self.id = id
        self.text = text
        self.fg = fg
        self.bg = bg
        self.bold = bold
        self.dim = dim
        self.inverse = inverse
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
        case .esc: return ["esc"]
        case .tab: return ["tab"]
        case .shiftTab: return ["shift+tab"]
        case .up: return ["up"]
        case .down: return ["down"]
        case .left: return ["left"]
        case .right: return ["right"]
        case .enter: return ["enter"]
        case .backspace: return ["backspace"]
        case .slash: return ["/"]
        case .ctrlC: return ["ctrl+c"]
        case .ctrlD: return ["ctrl+d"]
        case .ctrlBackslash: return ["ctrl+\\"]
        }
    }

    public var encodedBytes: Data {
        GhosttyReflowEngine.encodeKey(self)
    }
}

/// Stateful VT100/xterm terminal surface engine supporting streaming UTF-8 bytes,
/// CJK wide characters, 24-bit TrueColor SGR (fg + bg + inverse), alternate screen (`?1049h`/`?47h`),
/// `OSC 7501` (kv & JSON) / `OSC 0/2`, scrollback reflow, and Git diff classification.
public struct GhosttyReflowEngine: GhosttyVTSurface {
    private struct StyledCell: Sendable, Equatable {
        var ch: Character
        var fg: GhosttyRGBColor?
        var bg: GhosttyRGBColor?
        var bold: Bool
        var dim: Bool
        var inverse: Bool
        var width: UInt8 // 1 for normal, 2 for wide lead, 0 for continuation/empty
        var isWideContinuation: Bool

        static let blank = StyledCell(
            ch: " ",
            fg: nil,
            bg: nil,
            bold: false,
            dim: false,
            inverse: false,
            width: 1,
            isWideContinuation: false
        )

        var isDefaultBlank: Bool {
            ch == " " && !isWideContinuation && width == 1 && fg == nil && bg == nil && !bold && !dim && !inverse
        }
    }

    private struct ScreenRow: Sendable, Equatable {
        var cells: [StyledCell]
        var softWrapped: Bool

        init(columns: Int, softWrapped: Bool = false) {
            self.cells = Array(repeating: .blank, count: max(1, columns))
            self.softWrapped = softWrapped
        }

        mutating func clear(fg: GhosttyRGBColor? = nil, bg: GhosttyRGBColor? = nil, bold: Bool = false, dim: Bool = false) {
            let fill = StyledCell(ch: " ", fg: fg, bg: bg, bold: bold, dim: dim, inverse: false, width: 1, isWideContinuation: false)
            cells = Array(repeating: fill, count: cells.count)
            softWrapped = false
        }

        var trimmedEndIndex: Int {
            var end = cells.count
            while end > 0 {
                let c = cells[end - 1]
                if c.ch == " " && !c.isWideContinuation && c.fg == nil && c.bg == nil && !c.bold && !c.dim && !c.inverse {
                    end -= 1
                } else {
                    break
                }
            }
            return end
        }

        var plainTrimmedString: String {
            var end = cells.count
            while end > 0 && cells[end - 1].ch == " " && !cells[end - 1].isWideContinuation {
                end -= 1
            }
            var out = ""
            out.reserveCapacity(end)
            for idx in 0..<end {
                if !cells[idx].isWideContinuation {
                    out.append(cells[idx].ch)
                }
            }
            return out
        }
    }

    private struct CursorState: Sendable {
        var row: Int = 0
        var col: Int = 0
        var fg: GhosttyRGBColor? = nil
        var bg: GhosttyRGBColor? = nil
        var bold: Bool = false
        var dim: Bool = false
        var inverse: Bool = false
        var pendingWrap: Bool = false
    }

    private enum VTParseState: Sendable {
        case ground
        case escape
        case escapeIntermediate
        case csi
        case osc
        case oscEscape
        case stringSkip // DCS / APC / PM terminated by BEL or ST
        case stringSkipEscape
    }

    public private(set) var columns: Int
    public private(set) var rows: Int
    public private(set) var visibleLines: [String] = []
    public private(set) var styledLines: [GhosttyStyledLine] = []
    public private(set) var plainVisibleText: String = ""
    public private(set) var title: String = ""
    public private(set) var inAltScreen: Bool = false
    public private(set) var bracketedPasteEnabled: Bool = false
    public private(set) var bracketedPasteExplicit: Bool = false
    public private(set) var mouseTrackingMode: Int = 0
    public private(set) var mouseTrackingExplicit: Bool = false
    public private(set) var sgrMouseEncoding: Bool = false
    public private(set) var sgrMouseExplicit: Bool = false
    public private(set) var latestHerdrStatus: HerdrOSC7501Status?
    /// Monotonically incremented only when a fresh OSC 7501 payload is parsed (or engine resets),
    /// preventing callers from re-applying sticky `latestHerdrStatus` on plain stdout chunks.
    public private(set) var oscStatusGeneration: UInt64 = 0

    private let maxScrollback: Int
    private var primaryGrid: [ScreenRow]
    private var primaryScrollback: [ScreenRow] = []
    private var altGrid: [ScreenRow]

    private var cursorRow: Int = 0
    private var cursorCol: Int = 0
    private var pendingWrap: Bool = false
    private var savedPrimaryCursor = CursorState()
    private var savedAltCursor = CursorState()

    private var scrollTop: Int = 0
    private var scrollBottom: Int

    private var curFg: GhosttyRGBColor? = nil
    private var curBg: GhosttyRGBColor? = nil
    private var curBold: Bool = false
    private var curDim: Bool = false
    private var curInverse: Bool = false
    public var treatLFAsCRLF: Bool

    private var parseState: VTParseState = .ground
    private var csiBuffer: [UInt8] = []
    private var oscBuffer: [UnicodeScalar] = []
    private var pendingUTF8: [UInt8] = []

    private static let maxCSIBytes = 256
    private static let maxOSCScalars = 4096

    public init(columns: Int = 52, rows: Int = 24, maxScrollback: Int = 1500, treatLFAsCRLF: Bool = false) {
        let cols = max(20, columns)
        let rws = max(6, rows)
        self.columns = cols
        self.rows = rws
        self.maxScrollback = max(0, maxScrollback)
        self.treatLFAsCRLF = treatLFAsCRLF
        self.primaryGrid = (0..<rws).map { _ in ScreenRow(columns: cols) }
        self.altGrid = (0..<rws).map { _ in ScreenRow(columns: cols) }
        self.scrollBottom = rws - 1
        rebuildDerivedOutputs()
    }

    public mutating func reset() {
        primaryGrid = (0..<rows).map { _ in ScreenRow(columns: columns) }
        primaryScrollback = []
        altGrid = (0..<rows).map { _ in ScreenRow(columns: columns) }
        inAltScreen = false
        bracketedPasteEnabled = false
        bracketedPasteExplicit = false
        mouseTrackingMode = 0
        mouseTrackingExplicit = false
        sgrMouseEncoding = false
        sgrMouseExplicit = false
        cursorRow = 0
        cursorCol = 0
        pendingWrap = false
        savedPrimaryCursor = CursorState()
        savedAltCursor = CursorState()
        scrollTop = 0
        scrollBottom = rows - 1
        curFg = nil
        curBg = nil
        curBold = false
        curDim = false
        curInverse = false
        parseState = .ground
        csiBuffer.removeAll(keepingCapacity: true)
        oscBuffer.removeAll(keepingCapacity: true)
        pendingUTF8.removeAll(keepingCapacity: true)
        title = ""
        latestHerdrStatus = nil
        oscStatusGeneration &+= 1
        rebuildDerivedOutputs()
    }

    public mutating func feed(_ utf8Bytes: Data) {
        guard !utf8Bytes.isEmpty else { return }
        pendingUTF8.append(contentsOf: utf8Bytes)
        let buf = pendingUTF8
        pendingUTF8.removeAll(keepingCapacity: true)

        var idx = 0
        let total = buf.count
        while idx < total {
            let b = buf[idx]
            if b < 0x80 {
                processScalar(UnicodeScalar(b))
                idx += 1
                continue
            }

            let expectedLen: Int
            if b >= 0xC2 && b <= 0xDF {
                expectedLen = 2
            } else if b >= 0xE0 && b <= 0xEF {
                expectedLen = 3
            } else if b >= 0xF0 && b <= 0xF4 {
                expectedLen = 4
            } else {
                // Orphan continuation byte or illegal UTF-8 lead byte
                processScalar(UnicodeScalar(0xFFFD)!)
                idx += 1
                continue
            }

            if idx + expectedLen > total {
                // Incomplete multi-byte UTF-8 sequence split across chunk boundary
                break
            }

            let slice = buf[idx..<(idx + expectedLen)]
            var decoder = UTF8()
            var iter = slice.makeIterator()
            switch decoder.decode(&iter) {
            case .scalarValue(let scalar):
                processScalar(scalar)
                idx += expectedLen
            case .emptyInput, .error:
                processScalar(UnicodeScalar(0xFFFD)!)
                idx += 1
            }
        }

        if idx < total {
            pendingUTF8.append(contentsOf: buf[idx..<total])
        }
        rebuildDerivedOutputs()
    }

    public mutating func feedBase64PtyChunk(_ dataB64: String) {
        guard let raw = Data(base64Encoded: dataB64) else { return }
        feed(raw)
    }

    /// Preserves prior terminal private-mode flags (`?2004h`/`?2004l`, mouse reporting, SGR mouse)
    /// across plain-text `readTerminalScreen` snapshot reloads when the snapshot itself didn't emit them.
    public mutating func restoreModeFlagsIfUnset(from prior: GhosttyReflowEngine) {
        if !bracketedPasteExplicit {
            bracketedPasteEnabled = prior.bracketedPasteEnabled
            bracketedPasteExplicit = prior.bracketedPasteExplicit
        }
        if !mouseTrackingExplicit {
            mouseTrackingMode = prior.mouseTrackingMode
            mouseTrackingExplicit = prior.mouseTrackingExplicit
        }
        if !sgrMouseExplicit {
            sgrMouseEncoding = prior.sgrMouseEncoding
            sgrMouseExplicit = prior.sgrMouseExplicit
        }
    }

    public mutating func resizeReflow(columns newColsRaw: Int, rows newRowsRaw: Int) {
        let newCols = max(20, newColsRaw)
        let newRows = max(6, newRowsRaw)
        if newCols == columns && newRows == rows { return }

        if inAltScreen {
            altGrid = resizeGridWithoutReflow(altGrid, newCols: newCols, newRows: newRows)
            let savedAltRow = cursorRow
            let savedAltCol = cursorCol
            cursorRow = min(max(0, savedPrimaryCursor.row), max(0, rows - 1))
            cursorCol = min(max(0, savedPrimaryCursor.col), max(0, columns - 1))
            let reflowed = reflowPrimaryHistoryAndGrid(newCols: newCols, newRows: newRows)
            primaryScrollback = reflowed.scrollback
            primaryGrid = reflowed.grid
            savedPrimaryCursor.row = min(max(0, reflowed.cursorRow), newRows - 1)
            savedPrimaryCursor.col = min(max(0, reflowed.cursorCol), newCols - 1)
            cursorRow = savedAltRow
            cursorCol = savedAltCol
        } else {
            let reflowed = reflowPrimaryHistoryAndGrid(newCols: newCols, newRows: newRows)
            primaryScrollback = reflowed.scrollback
            primaryGrid = reflowed.grid
            altGrid = resizeGridWithoutReflow(altGrid, newCols: newCols, newRows: newRows)
            cursorRow = min(max(0, reflowed.cursorRow), newRows - 1)
            cursorCol = min(max(0, reflowed.cursorCol), newCols - 1)
        }

        columns = newCols
        rows = newRows
        cursorRow = min(max(0, cursorRow), newRows - 1)
        cursorCol = min(max(0, cursorCol), newCols - 1)
        pendingWrap = false
        scrollTop = 0
        scrollBottom = newRows - 1
        rebuildDerivedOutputs()
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

    public static func encodeMouseWheel(
        up: Bool,
        column: Int = 1,
        row: Int = 1,
        sgrEncoding: Bool = true
    ) -> Data {
        let btn = up ? 64 : 65
        let col = max(1, min(9999, column))
        let rw = max(1, min(9999, row))
        if sgrEncoding {
            return Data("\u{1B}[<\(btn);\(col);\(rw)M".utf8)
        }
        let cb = UInt8(clamping: 32 + btn)
        let cx = UInt8(clamping: 32 + min(223, col))
        let cy = UInt8(clamping: 32 + min(223, rw))
        return Data([0x1B, 0x5B, 0x4D, cb, cx, cy])
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

    // MARK: - Rich ANSI / Diff Rendering for Snapshots & Views

    public static func renderStyledLines(
        rawOutput: String,
        targetColumns: Int
    ) -> (lines: [GhosttyStyledLine], latestStatus: HerdrOSC7501Status?) {
        var engine = GhosttyReflowEngine(
            columns: max(24, targetColumns),
            rows: 24,
            maxScrollback: 2000,
            treatLFAsCRLF: true
        )
        engine.feed(Data(rawOutput.utf8))
        return (engine.styledLines, engine.latestHerdrStatus)
    }

    // MARK: - Streaming VT State Machine

    private mutating func processScalar(_ scalar: UnicodeScalar) {
        switch parseState {
        case .ground:
            handleGroundScalar(scalar)

        case .escape:
            handleEscapeScalar(scalar)

        case .escapeIntermediate:
            if scalar.value >= 0x30 && scalar.value <= 0x7E {
                parseState = .ground
            } else if scalar.value == 0x1B {
                parseState = .escape
            } else if scalar.value < 0x20 || scalar.value >= 0x80 {
                parseState = .ground
            }

        case .csi:
            let v = scalar.value
            if v == 0x1B {
                csiBuffer.removeAll(keepingCapacity: true)
                parseState = .escape
            } else if v >= 0x40 && v <= 0x7E {
                executeCSI(finalByte: UInt8(v))
                csiBuffer.removeAll(keepingCapacity: true)
                parseState = .ground
            } else if v >= 0x20 && v <= 0x3F {
                if csiBuffer.count < Self.maxCSIBytes {
                    csiBuffer.append(UInt8(v))
                }
            } else if v < 0x20 {
                handleC0InEscape(scalar)
            }

        case .osc:
            if scalar.value == 0x07 {
                finalizeOSC()
                oscBuffer.removeAll(keepingCapacity: true)
                parseState = .ground
            } else if scalar.value == 0x1B {
                parseState = .oscEscape
            } else if oscBuffer.count < Self.maxOSCScalars {
                oscBuffer.append(scalar)
            }

        case .oscEscape:
            if scalar.value == 0x5C { // '\' -> ST
                finalizeOSC()
                oscBuffer.removeAll(keepingCapacity: true)
                parseState = .ground
            } else {
                finalizeOSC()
                oscBuffer.removeAll(keepingCapacity: true)
                parseState = .escape
                handleEscapeScalar(scalar)
            }

        case .stringSkip:
            if scalar.value == 0x07 {
                parseState = .ground
            } else if scalar.value == 0x1B {
                parseState = .stringSkipEscape
            }

        case .stringSkipEscape:
            if scalar.value == 0x5C {
                parseState = .ground
            } else {
                parseState = .escape
                handleEscapeScalar(scalar)
            }
        }
    }

    private mutating func handleC0InEscape(_ scalar: UnicodeScalar) {
        switch scalar.value {
        case 0x08: backspace()
        case 0x09: horizontalTab()
        case 0x0A, 0x0B, 0x0C: lineFeed()
        case 0x0D: carriageReturn()
        default: break
        }
    }

    private mutating func handleGroundScalar(_ scalar: UnicodeScalar) {
        switch scalar.value {
        case 0x07:
            break
        case 0x08:
            backspace()
        case 0x09:
            horizontalTab()
        case 0x0A, 0x0B, 0x0C:
            lineFeed()
        case 0x0D:
            carriageReturn()
        case 0x1B:
            parseState = .escape
        case 0x00..<0x20, 0x7F:
            break
        default:
            putScalar(scalar)
        }
    }

    private mutating func handleEscapeScalar(_ scalar: UnicodeScalar) {
        switch scalar.value {
        case 0x5B: // '[' CSI
            csiBuffer.removeAll(keepingCapacity: true)
            parseState = .csi
        case 0x5D: // ']' OSC
            oscBuffer.removeAll(keepingCapacity: true)
            parseState = .osc
        case 0x50, 0x58, 0x5E, 0x5F: // DCS, SOS, PM, APC
            parseState = .stringSkip
        case 0x20...0x2F: // nF / charset designation like ESC ( B, ESC ) 0
            parseState = .escapeIntermediate
        case 0x37: // '7' DECSC
            saveCursor()
            parseState = .ground
        case 0x38: // '8' DECRC
            restoreCursor()
            parseState = .ground
        case 0x44: // 'D' IND
            lineFeedStrict()
            parseState = .ground
        case 0x45: // 'E' NEL
            carriageReturn()
            lineFeedStrict()
            parseState = .ground
        case 0x4D: // 'M' RI
            reverseIndex()
            parseState = .ground
        case 0x63: // 'c' RIS
            reset()
        default:
            parseState = .ground
        }
    }

    // MARK: - Grid Operations

    private mutating func withActiveGrid<R>(_ body: (inout [ScreenRow]) -> R) -> R {
        if inAltScreen {
            return body(&altGrid)
        } else {
            return body(&primaryGrid)
        }
    }

    private mutating func carriageReturn() {
        cursorCol = 0
        pendingWrap = false
    }

    private mutating func backspace() {
        pendingWrap = false
        if cursorCol > 0 {
            cursorCol -= 1
        }
    }

    private mutating func horizontalTab() {
        pendingWrap = false
        let nextStop = ((cursorCol / 8) + 1) * 8
        cursorCol = min(columns - 1, nextStop)
    }

    private mutating func lineFeed() {
        lineFeedStrict()
        if treatLFAsCRLF && !inAltScreen {
            cursorCol = 0
        }
    }

    private mutating func lineFeedStrict() {
        pendingWrap = false
        if cursorRow == scrollBottom {
            scrollUp(lines: 1)
        } else if cursorRow < rows - 1 {
            cursorRow += 1
        }
    }

    private mutating func reverseIndex() {
        pendingWrap = false
        if cursorRow == scrollTop {
            scrollDown(lines: 1)
        } else if cursorRow > 0 {
            cursorRow -= 1
        }
    }

    private mutating func scrollUp(lines: Int) {
        let n = max(1, min(lines, scrollBottom - scrollTop + 1))
        let captureScrollback = !inAltScreen && scrollTop == 0 && maxScrollback > 0
        let blankRow = ScreenRow(columns: columns)
        let top = scrollTop
        let bottom = scrollBottom

        if captureScrollback {
            for r in top..<(top + n) where r < primaryGrid.count {
                primaryScrollback.append(primaryGrid[r])
            }
            if primaryScrollback.count > maxScrollback {
                primaryScrollback.removeFirst(primaryScrollback.count - maxScrollback)
            }
        }

        withActiveGrid { grid in
            for r in top..<(bottom - n + 1) {
                grid[r] = grid[r + n]
            }
            for r in (bottom - n + 1)...bottom {
                grid[r] = blankRow
            }
        }
    }

    private mutating func scrollDown(lines: Int) {
        let n = max(1, min(lines, scrollBottom - scrollTop + 1))
        let blankRow = ScreenRow(columns: columns)
        let top = scrollTop
        let bottom = scrollBottom
        withActiveGrid { grid in
            var r = bottom
            while r >= top + n {
                grid[r] = grid[r - n]
                r -= 1
            }
            for fill in top..<(top + n) {
                grid[fill] = blankRow
            }
        }
    }

    private mutating func saveCursor() {
        let snapshot = CursorState(
            row: cursorRow,
            col: cursorCol,
            fg: curFg,
            bg: curBg,
            bold: curBold,
            dim: curDim,
            inverse: curInverse,
            pendingWrap: pendingWrap
        )
        if inAltScreen {
            savedAltCursor = snapshot
        } else {
            savedPrimaryCursor = snapshot
        }
    }

    private mutating func restoreCursor() {
        let snapshot = inAltScreen ? savedAltCursor : savedPrimaryCursor
        cursorRow = min(max(0, snapshot.row), rows - 1)
        cursorCol = min(max(0, snapshot.col), columns - 1)
        curFg = snapshot.fg
        curBg = snapshot.bg
        curBold = snapshot.bold
        curDim = snapshot.dim
        curInverse = snapshot.inverse
        pendingWrap = snapshot.pendingWrap
    }

    private mutating func enterAltScreen(saveCursorFirst: Bool, clearAlt: Bool) {
        if saveCursorFirst {
            savedPrimaryCursor = CursorState(
                row: cursorRow,
                col: cursorCol,
                fg: curFg,
                bg: curBg,
                bold: curBold,
                dim: curDim,
                inverse: curInverse,
                pendingWrap: pendingWrap
            )
        }
        inAltScreen = true
        if clearAlt || altGrid.count != rows {
            altGrid = (0..<rows).map { _ in ScreenRow(columns: columns) }
        }
        cursorRow = 0
        cursorCol = 0
        pendingWrap = false
        scrollTop = 0
        scrollBottom = rows - 1
    }

    private mutating func leaveAltScreen(restoreSavedCursor: Bool) {
        inAltScreen = false
        scrollTop = 0
        scrollBottom = rows - 1
        pendingWrap = false
        if restoreSavedCursor {
            restoreCursor()
        } else {
            cursorRow = min(cursorRow, rows - 1)
            cursorCol = min(cursorCol, columns - 1)
        }
    }

    private static func clearWideNeighbors(in row: inout ScreenRow, at col: Int) {
        guard col >= 0 && col < row.cells.count else { return }
        if row.cells[col].isWideContinuation && col > 0 {
            row.cells[col - 1] = .blank
        } else if row.cells[col].width == 2 && col + 1 < row.cells.count {
            row.cells[col + 1] = .blank
        }
        row.cells[col] = .blank
    }

    private mutating func putScalar(_ scalar: UnicodeScalar) {
        let w = Self.scalarDisplayWidth(scalar)
        if w == 0 {
            // Combining scalar: attach to previous printable cell if possible
            let targetCol: Int? = if pendingWrap {
                columns - 1
            } else if cursorCol > 0 {
                cursorCol - 1
            } else {
                nil
            }
            if let tc = targetCol {
                let r = cursorRow
                withActiveGrid { grid in
                    var actualCol = tc
                    if grid[r].cells[actualCol].isWideContinuation && actualCol > 0 {
                        actualCol -= 1
                    }
                    let existing = String(grid[r].cells[actualCol].ch)
                    if let combined = (existing + String(scalar)).first {
                        grid[r].cells[actualCol].ch = combined
                    }
                }
            }
            return
        }

        if pendingWrap {
            let r = cursorRow
            withActiveGrid { grid in
                grid[r].softWrapped = true
            }
            cursorCol = 0
            pendingWrap = false
            lineFeedStrict()
        }

        if w == 2 && cursorCol >= columns - 1 {
            let r = cursorRow
            let col = cursorCol
            withActiveGrid { grid in
                Self.clearWideNeighbors(in: &grid[r], at: col)
                grid[r].softWrapped = true
            }
            cursorCol = 0
            lineFeedStrict()
        }

        let ch = Character(scalar)
        let fg = curFg
        let bg = curBg
        let bold = curBold
        let dim = curDim
        let inverse = curInverse
        let r = cursorRow
        let col = cursorCol
        let cols = columns

        withActiveGrid { grid in
            Self.clearWideNeighbors(in: &grid[r], at: col)
            if w == 2 && col + 1 < cols {
                Self.clearWideNeighbors(in: &grid[r], at: col + 1)
                grid[r].cells[col] = StyledCell(
                    ch: ch, fg: fg, bg: bg, bold: bold, dim: dim, inverse: inverse, width: 2, isWideContinuation: false
                )
                grid[r].cells[col + 1] = StyledCell(
                    ch: " ", fg: fg, bg: bg, bold: bold, dim: dim, inverse: inverse, width: 0, isWideContinuation: true
                )
            } else {
                grid[r].cells[col] = StyledCell(
                    ch: ch, fg: fg, bg: bg, bold: bold, dim: dim, inverse: inverse, width: 1, isWideContinuation: false
                )
            }
        }

        if cursorCol + w >= columns {
            cursorCol = columns - 1
            pendingWrap = true
        } else {
            cursorCol += w
        }
    }

    // MARK: - CSI & OSC Execution

    private mutating func executeCSI(finalByte: UInt8) {
        let raw = String(decoding: csiBuffer, as: UTF8.self)
        var prefix: Character? = nil
        var body = raw
        if let first = raw.first, first == "?" || first == ">" || first == "<" || first == "=" {
            prefix = first
            body = String(raw.dropFirst())
        }
        let hasIntermediates = body.unicodeScalars.contains { $0.value >= 0x20 && $0.value <= 0x2F }
        // Ignore non-`?` private prefixes (`>`, `<`, `=`) and intermediate sequences (`CSI > 1 u`, `CSI 2 q`, `CSI > 4 ; 2 m`)
        if hasIntermediates || (prefix != nil && prefix != "?") {
            return
        }
        let isPrivate = (prefix == "?")
        let cleanParams = body

        // Preserve colon sub-parameters for SGR (`38:2::r:g:b`)
        if finalByte == 0x6D /* 'm' */ && !isPrivate {
            Self.applySGR(params: cleanParams, fg: &curFg, bg: &curBg, bold: &curBold, dim: &curDim, inverse: &curInverse)
            return
        }

        let ints: [Int] = cleanParams.isEmpty
            ? []
            : cleanParams.split(separator: ";", omittingEmptySubsequences: false).map { Int($0) ?? 0 }
        let p1 = ints.first ?? 0
        let count1 = max(1, min(p1, 65_535))

        if isPrivate {
            switch finalByte {
            case 0x68: // 'h' DECSET
                for code in ints {
                    if code == 1049 || code == 1047 || code == 47 {
                        enterAltScreen(saveCursorFirst: code == 1049, clearAlt: true)
                    } else if code == 2004 {
                        bracketedPasteEnabled = true
                        bracketedPasteExplicit = true
                    } else if code == 1000 || code == 1002 || code == 1003 {
                        mouseTrackingMode = code
                        mouseTrackingExplicit = true
                    } else if code == 1006 {
                        sgrMouseEncoding = true
                        sgrMouseExplicit = true
                    }
                }
            case 0x6C: // 'l' DECRST
                for code in ints {
                    if code == 1049 || code == 1047 || code == 47 {
                        leaveAltScreen(restoreSavedCursor: code == 1049)
                    } else if code == 2004 {
                        bracketedPasteEnabled = false
                        bracketedPasteExplicit = true
                    } else if code == 1000 || code == 1002 || code == 1003 {
                        if mouseTrackingMode == code {
                            mouseTrackingMode = 0
                        }
                        mouseTrackingExplicit = true
                    } else if code == 1006 {
                        sgrMouseEncoding = false
                        sgrMouseExplicit = true
                    }
                }
            default:
                break
            }
            return
        }

        switch finalByte {
        case 0x41: // 'A' CUU
            pendingWrap = false
            cursorRow = max(0, cursorRow - count1)
        case 0x42, 0x65: // 'B' CUD / 'e' VPR
            pendingWrap = false
            cursorRow = min(rows - 1, cursorRow + count1)
        case 0x43, 0x61: // 'C' CUF / 'a' HPR
            pendingWrap = false
            cursorCol = min(columns - 1, cursorCol + count1)
        case 0x44: // 'D' CUB
            pendingWrap = false
            cursorCol = max(0, cursorCol - count1)
        case 0x45: // 'E' CNL
            pendingWrap = false
            cursorRow = min(rows - 1, cursorRow + count1)
            cursorCol = 0
        case 0x46: // 'F' CPL
            pendingWrap = false
            cursorRow = max(0, cursorRow - count1)
            cursorCol = 0
        case 0x47, 0x60: // 'G' CHA / '`' HPA
            pendingWrap = false
            cursorCol = min(columns - 1, max(0, count1 - 1))
        case 0x48, 0x66: // 'H' CUP / 'f' HVP
            pendingWrap = false
            let r = max(1, min(ints.count >= 1 ? ints[0] : 1, 65_535))
            let c = max(1, min(ints.count >= 2 ? ints[1] : 1, 65_535))
            cursorRow = min(rows - 1, r - 1)
            cursorCol = min(columns - 1, c - 1)
        case 0x64: // 'd' VPA
            pendingWrap = false
            cursorRow = min(rows - 1, max(0, count1 - 1))
        case 0x4A: // 'J' ED
            eraseInDisplay(mode: p1)
        case 0x4B: // 'K' EL
            eraseInLine(mode: p1)
        case 0x58: // 'X' ECH
            let cols = columns
            let r = cursorRow
            let start = cursorCol
            let end = min(cols, start + count1)
            withActiveGrid { grid in
                for c in start..<end {
                    Self.clearWideNeighbors(in: &grid[r], at: c)
                }
            }
        case 0x4C: // 'L' IL (insert lines inside scroll region)
            if cursorRow >= scrollTop && cursorRow <= scrollBottom {
                let savedTop = scrollTop
                scrollTop = cursorRow
                scrollDown(lines: count1)
                scrollTop = savedTop
            }
        case 0x4D: // 'M' DL (delete lines inside scroll region)
            if cursorRow >= scrollTop && cursorRow <= scrollBottom {
                let savedTop = scrollTop
                scrollTop = cursorRow
                scrollUp(lines: count1)
                scrollTop = savedTop
            }
        case 0x40: // '@' ICH (insert blank characters)
            let cols = columns
            let r = cursorRow
            let col = cursorCol
            let n = min(count1, cols - col)
            withActiveGrid { grid in
                Self.clearWideNeighbors(in: &grid[r], at: col)
                var c = cols - 1
                while c >= col + n {
                    grid[r].cells[c] = grid[r].cells[c - n]
                    c -= 1
                }
                for fill in col..<(col + n) {
                    grid[r].cells[fill] = .blank
                }
                if cols > 0 && grid[r].cells[cols - 1].width == 2 {
                    grid[r].cells[cols - 1] = .blank
                }
            }
        case 0x50: // 'P' DCH (delete characters)
            let cols = columns
            let r = cursorRow
            let col = cursorCol
            let n = min(count1, cols - col)
            withActiveGrid { grid in
                Self.clearWideNeighbors(in: &grid[r], at: col)
                for c in col..<(cols - n) {
                    grid[r].cells[c] = grid[r].cells[c + n]
                }
                for fill in (cols - n)..<cols {
                    grid[r].cells[fill] = .blank
                }
                if grid[r].cells[col].isWideContinuation {
                    grid[r].cells[col] = .blank
                }
            }
        case 0x53: // 'S' SU
            scrollUp(lines: count1)
        case 0x54: // 'T' SD
            scrollDown(lines: count1)
        case 0x72: // 'r' DECSTBM
            let top = max(1, ints.count >= 1 && ints[0] > 0 ? ints[0] : 1)
            let bot = min(rows, ints.count >= 2 && ints[1] > 0 ? ints[1] : rows)
            if top < bot {
                scrollTop = top - 1
                scrollBottom = bot - 1
            } else {
                scrollTop = 0
                scrollBottom = rows - 1
            }
            cursorRow = 0
            cursorCol = 0
            pendingWrap = false
        case 0x73: // 's' SCOSC
            saveCursor()
        case 0x75: // 'u' SCORC
            restoreCursor()
        default:
            break
        }
    }

    private mutating func eraseInDisplay(mode: Int) {
        let cols = columns
        let rNow = cursorRow
        let cNow = cursorCol
        let totalRows = rows
        withActiveGrid { grid in
            switch mode {
            case 0:
                for c in cNow..<cols {
                    Self.clearWideNeighbors(in: &grid[rNow], at: c)
                }
                grid[rNow].softWrapped = false
                for r in (rNow + 1)..<totalRows {
                    grid[r].clear()
                }
            case 1:
                for r in 0..<rNow {
                    grid[r].clear()
                }
                for c in 0...min(cNow, cols - 1) {
                    Self.clearWideNeighbors(in: &grid[rNow], at: c)
                }
            case 2, 3:
                for r in 0..<totalRows {
                    grid[r].clear()
                }
            default:
                break
            }
        }
        if mode == 3 && !inAltScreen {
            primaryScrollback.removeAll(keepingCapacity: true)
        }
    }

    private mutating func eraseInLine(mode: Int) {
        let cols = columns
        let r = cursorRow
        let cNow = cursorCol
        withActiveGrid { grid in
            switch mode {
            case 0:
                for c in cNow..<cols {
                    Self.clearWideNeighbors(in: &grid[r], at: c)
                }
                grid[r].softWrapped = false
            case 1:
                for c in 0...min(cNow, cols - 1) {
                    Self.clearWideNeighbors(in: &grid[r], at: c)
                }
            case 2:
                grid[r].clear()
            default:
                break
            }
        }
    }

    private mutating func finalizeOSC() {
        let raw = String(String.UnicodeScalarView(oscBuffer))
        if raw.hasPrefix("7501;") {
            let payload = String(raw.dropFirst(5))
            if let status = Self.parseOSC7501Payload(payload, separator: ";") {
                latestHerdrStatus = status
                oscStatusGeneration &+= 1
            }
        } else if raw.hasPrefix("7501:") {
            let payload = String(raw.dropFirst(5))
            if let status = Self.parseOSC7501Payload(payload, separator: ":") {
                latestHerdrStatus = status
                oscStatusGeneration &+= 1
            }
        } else if raw.hasPrefix("0;") || raw.hasPrefix("2;") {
            title = String(raw.dropFirst(2))
        }
    }

    // MARK: - Resize Reflow & Output Materialization

    private func resizeGridWithoutReflow(
        _ grid: [ScreenRow],
        newCols: Int,
        newRows: Int
    ) -> [ScreenRow] {
        var out = (0..<newRows).map { _ in ScreenRow(columns: newCols) }
        let rowCount = min(grid.count, newRows)
        for r in 0..<rowCount {
            let copyCols = min(grid[r].cells.count, newCols)
            for c in 0..<copyCols {
                out[r].cells[c] = grid[r].cells[c]
            }
            if newCols > 0 && copyCols < grid[r].cells.count && out[r].cells[newCols - 1].width == 2 {
                out[r].cells[newCols - 1] = .blank
            }
        }
        return out
    }

    private func reflowPrimaryHistoryAndGrid(
        newCols: Int,
        newRows: Int
    ) -> (scrollback: [ScreenRow], grid: [ScreenRow], cursorRow: Int, cursorCol: Int) {
        var combined = primaryScrollback + primaryGrid
        // Drop trailing blank lines beyond cursor row
        let cursorAbs = primaryScrollback.count + cursorRow
        while combined.count > cursorAbs + 1 && combined.last?.trimmedEndIndex == 0 && !combined.last!.softWrapped {
            combined.removeLast()
        }

        struct LogicalLine {
            var cells: [StyledCell]
            var containsCursor: Bool
            var cursorOffset: Int
        }

        var logicalLines: [LogicalLine] = []
        var idx = 0
        while idx < combined.count {
            var cells: [StyledCell] = []
            var hasCursor = false
            var curOffset = 0
            while true {
                let row = combined[idx]
                let isCursorRow = (idx == cursorAbs)
                let endIdx = row.softWrapped
                    ? row.cells.count
                    : max(row.trimmedEndIndex, isCursorRow ? min(cursorCol, row.cells.count) : 0)
                if isCursorRow {
                    hasCursor = true
                    curOffset = cells.count + min(cursorCol, row.cells.count)
                }
                for c in 0..<min(endIdx, row.cells.count) {
                    if !row.cells[c].isWideContinuation {
                        cells.append(row.cells[c])
                    } else if isCursorRow && c < cursorCol {
                        curOffset = max(0, curOffset - 1)
                    }
                }
                let cont = row.softWrapped && idx + 1 < combined.count
                idx += 1
                if !cont { break }
            }
            logicalLines.append(LogicalLine(cells: cells, containsCursor: hasCursor, cursorOffset: curOffset))
        }

        var wrappedRows: [ScreenRow] = []
        var newCursorAbs = 0
        var newCursorCol = 0

        for line in logicalLines {
            if line.cells.isEmpty {
                if line.containsCursor {
                    newCursorAbs = wrappedRows.count
                    newCursorCol = 0
                }
                wrappedRows.append(ScreenRow(columns: newCols, softWrapped: false))
                continue
            }

            var currentRow = ScreenRow(columns: newCols, softWrapped: false)
            var col = 0
            for (cellIdx, cell) in line.cells.enumerated() {
                let w = cell.width == 2 ? 2 : 1
                if col + w > newCols {
                    currentRow.softWrapped = true
                    wrappedRows.append(currentRow)
                    currentRow = ScreenRow(columns: newCols, softWrapped: false)
                    col = 0
                }
                if line.containsCursor && cellIdx == line.cursorOffset {
                    newCursorAbs = wrappedRows.count
                    newCursorCol = col
                }
                currentRow.cells[col] = cell
                if w == 2 && col + 1 < newCols {
                    currentRow.cells[col + 1] = StyledCell(
                        ch: " ", fg: cell.fg, bg: cell.bg, bold: cell.bold, dim: cell.dim, inverse: cell.inverse, width: 0, isWideContinuation: true
                    )
                }
                col += w
            }
            if line.containsCursor && line.cursorOffset >= line.cells.count {
                newCursorAbs = wrappedRows.count
                newCursorCol = min(col, newCols - 1)
            }
            wrappedRows.append(currentRow)
        }

        while wrappedRows.count < newRows {
            wrappedRows.append(ScreenRow(columns: newCols, softWrapped: false))
        }

        let sbCount = max(0, wrappedRows.count - newRows)
        let scrollbackSlice = sbCount > 0
            ? Array(wrappedRows.prefix(sbCount).suffix(maxScrollback))
            : []
        let gridSlice = Array(wrappedRows.suffix(newRows))
        let adjustedCursorRow = max(0, min(newRows - 1, newCursorAbs - sbCount))
        return (scrollbackSlice, gridSlice, adjustedCursorRow, min(newCols - 1, newCursorCol))
    }

    private mutating func rebuildDerivedOutputs() {
        let activeGrid = inAltScreen ? altGrid : primaryGrid
        var vis = activeGrid.map(\.plainTrimmedString)
        while vis.count > 1 && vis.last?.isEmpty == true {
            vis.removeLast()
        }
        visibleLines = vis
        plainVisibleText = vis.joined(separator: "\n")

        let sourceRows = inAltScreen ? altGrid : (primaryScrollback + primaryGrid)
        var nonTrailingBlankCount = sourceRows.count
        while nonTrailingBlankCount > 1 && sourceRows[nonTrailingBlankCount - 1].trimmedEndIndex == 0 {
            nonTrailingBlankCount -= 1
        }

        var linesOut: [GhosttyStyledLine] = []
        linesOut.reserveCapacity(nonTrailingBlankCount)
        for lineID in 0..<nonTrailingBlankCount {
            let row = sourceRows[lineID]
            let end = row.trimmedEndIndex
            if end == 0 {
                linesOut.append(GhosttyStyledLine(id: lineID, plainText: "", spans: [], kind: .normal))
                continue
            }
            let slice = row.cells[0..<end]
            var plain = ""
            plain.reserveCapacity(end)
            for cell in slice where !cell.isWideContinuation {
                plain.append(cell.ch)
            }
            let spans = Self.coalesceSpans(slice)
            let kind = Self.classifyLine(plain)
            linesOut.append(GhosttyStyledLine(id: lineID, plainText: plain, spans: spans, kind: kind))
        }
        styledLines = linesOut
    }

    // MARK: - Helpers (Span Coalescing, SGR, Wide Widths, Diff & OSC 7501)

    private static func coalesceSpans(_ cells: ArraySlice<StyledCell>) -> [GhosttyStyledSpan] {
        var spans: [GhosttyStyledSpan] = []
        var buf = ""
        var curFg: GhosttyRGBColor?
        var curBg: GhosttyRGBColor?
        var curBold = false
        var curDim = false
        var curInverse = false
        var spanId = 0

        for cell in cells {
            if cell.isWideContinuation { continue }
            if !buf.isEmpty && (cell.fg != curFg || cell.bg != curBg || cell.bold != curBold || cell.dim != curDim || cell.inverse != curInverse) {
                spans.append(GhosttyStyledSpan(id: spanId, text: buf, fg: curFg, bg: curBg, bold: curBold, dim: curDim, inverse: curInverse))
                spanId += 1
                buf = ""
            }
            if buf.isEmpty {
                curFg = cell.fg
                curBg = cell.bg
                curBold = cell.bold
                curDim = cell.dim
                curInverse = cell.inverse
            }
            buf.append(cell.ch)
        }
        if !buf.isEmpty {
            spans.append(GhosttyStyledSpan(id: spanId, text: buf, fg: curFg, bg: curBg, bold: curBold, dim: curDim, inverse: curInverse))
        }
        return spans
    }

    private static func scalarDisplayWidth(_ scalar: UnicodeScalar) -> Int {
        let v = scalar.value
        if v < 0x20 || v == 0x7F { return 0 }
        if (0x0300...0x036F).contains(v)
            || (0x1AB0...0x1AFF).contains(v)
            || (0x1DC0...0x1DFF).contains(v)
            || (0x200B...0x200D).contains(v)
            || (0x20D0...0x20FF).contains(v)
            || (0xFE00...0xFE0F).contains(v)
            || (0xFE20...0xFE2F).contains(v)
        {
            return 0
        }
        if (0x1100...0x115F).contains(v)
            || (0x2E80...0x9FFF).contains(v)
            || (0xAC00...0xD7AF).contains(v)
            || (0xF900...0xFAFF).contains(v)
            || (0xFE10...0xFE6F).contains(v)
            || (0xFF00...0xFF60).contains(v)
            || (0xFFE0...0xFFE6).contains(v)
            || (0x1F300...0x1FAFF).contains(v)
            || (0x20000...0x3FFFF).contains(v)
        {
            return 2
        }
        return 1
    }

    private static func applySGR(
        params: String,
        fg: inout GhosttyRGBColor?,
        bg: inout GhosttyRGBColor?,
        bold: inout Bool,
        dim: inout Bool,
        inverse: inout Bool
    ) {
        if params.isEmpty {
            fg = nil
            bg = nil
            bold = false
            dim = false
            inverse = false
            return
        }

        // Support both `;` and `:` (ISO-8613-6 e.g. `38:2::r:g:b`, `48:2::r:g:b`, `38:5:idx`, `48:5:idx`)
        let rawTokens = params.split(separator: ";", omittingEmptySubsequences: false)
        var idx = 0
        while idx < rawTokens.count {
            let tok = rawTokens[idx]
            if tok.contains(":") {
                let rawSub = tok.split(separator: ":", omittingEmptySubsequences: false)
                if let headStr = rawSub.first, let head = Int(headStr), (head == 38 || head == 48) {
                    let isFg = (head == 38)
                    let kind = rawSub.count >= 2 ? Int(rawSub[1]) : nil
                    if kind == 5 && rawSub.count >= 3, let n = Int(rawSub[2]) {
                        let c = xterm256Color(n)
                        if isFg { fg = c } else { bg = c }
                    } else if kind == 2 {
                        let off = rawSub.count >= 6 ? 3 : 2
                        if rawSub.count >= off + 3,
                           let r = Int(rawSub[off]),
                           let g = Int(rawSub[off + 1]),
                           let b = Int(rawSub[off + 2])
                        {
                            let c = GhosttyRGBColor(
                                r: UInt8(clamping: r),
                                g: UInt8(clamping: g),
                                b: UInt8(clamping: b)
                            )
                            if isFg { fg = c } else { bg = c }
                        }
                    }
                }
                idx += 1
                continue
            }

            let code = tok.isEmpty ? 0 : (Int(tok) ?? -1)
            switch code {
            case 0:
                fg = nil
                bg = nil
                bold = false
                dim = false
                inverse = false
            case 1:
                bold = true
            case 2:
                dim = true
            case 7:
                inverse = true
            case 22:
                bold = false
                dim = false
            case 27:
                inverse = false
            case 39:
                fg = nil
            case 49:
                bg = nil
            case 30...37:
                fg = standard16Color(code - 30)
            case 90...97:
                fg = standard16Color(8 + (code - 90))
            case 40...47:
                bg = standard16Color(code - 40)
            case 100...107:
                bg = standard16Color(8 + (code - 100))
            case 38, 48:
                let isFg = (code == 38)
                let next1 = idx + 1 < rawTokens.count ? Int(rawTokens[idx + 1]) : nil
                if next1 == 5, idx + 2 < rawTokens.count, let n = Int(rawTokens[idx + 2]) {
                    let c = xterm256Color(n)
                    if isFg { fg = c } else { bg = c }
                    idx += 2
                } else if next1 == 2,
                          idx + 4 < rawTokens.count,
                          let r = Int(rawTokens[idx + 2]),
                          let g = Int(rawTokens[idx + 3]),
                          let b = Int(rawTokens[idx + 4])
                {
                    let c = GhosttyRGBColor(
                        r: UInt8(clamping: r),
                        g: UInt8(clamping: g),
                        b: UInt8(clamping: b)
                    )
                    if isFg { fg = c } else { bg = c }
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

    private static let numberedDiffAddRegex = try! NSRegularExpression(pattern: #"^\s*\d+\s+\+"#)
    private static let numberedDiffDelRegex = try! NSRegularExpression(pattern: #"^\s*\d+\s+-"#)
    private static let promptChoiceRegex = try! NSRegularExpression(pattern: #"^(?:[>❯]\s*)?\d+\.\s+\S+"#)

    private static func matches(_ regex: NSRegularExpression, in text: String) -> Bool {
        let nsRange = NSRange(text.startIndex..<text.endIndex, in: text)
        return regex.firstMatch(in: text, options: [], range: nsRange) != nil
    }

    private static func classifyLine(_ line: String) -> GhosttyLineKind {
        let trimmed = line.trimmingCharacters(in: .whitespaces)
        if trimmed.hasPrefix("@@") || trimmed.hasPrefix("diff --git") {
            return .diffHunk
        }
        // Only classify actual unified diff additions/deletions (`+foo`, `+  indented`),
        // never single-space Markdown bullet lists (`- item`, `+ item`).
        let isMarkdownPlusBullet = line.hasPrefix("+ ") && !line.hasPrefix("+  ")
        let isMarkdownMinusBullet = line.hasPrefix("- ") && !line.hasPrefix("-  ")
        if (line.hasPrefix("+") && !line.hasPrefix("+++") && !isMarkdownPlusBullet)
            || matches(numberedDiffAddRegex, in: line)
        {
            return .diffAdded
        }
        if (line.hasPrefix("-") && !line.hasPrefix("---") && !isMarkdownMinusBullet)
            || matches(numberedDiffDelRegex, in: line)
        {
            return .diffRemoved
        }
        if matches(promptChoiceRegex, in: trimmed) {
            return .promptChoice
        }
        return .normal
    }

    private static func parseOSC7501Payload(_ payload: String, separator: Character) -> HerdrOSC7501Status? {
        let trimmed = payload.trimmingCharacters(in: .whitespacesAndNewlines)
        if trimmed.hasPrefix("{"),
           let data = trimmed.data(using: .utf8),
           let obj = try? JSONSerialization.jsonObject(with: data) as? [String: Any]
        {
            let state = HerdrLifecycleState.from(
                rawState: obj["state"] as? String,
                kind: obj["kind"] as? String
            )
            let progress = (obj["progress"] as? NSNumber)?.intValue
            return HerdrOSC7501Status(
                state: state,
                kind: obj["kind"] as? String,
                app: obj["app"] as? String,
                title: obj["title"] as? String,
                message: (obj["msg"] as? String) ?? (obj["task"] as? String),
                progress: progress
            )
        }
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
            let parts = payload.split(separator: ":", maxSplits: 1)
            let state = HerdrLifecycleState.from(rawState: parts.first.map(String.init))
            let msg = parts.count > 1 ? String(parts[1]) : nil
            return HerdrOSC7501Status(state: state, message: msg)
        }
        return nil
    }
}
