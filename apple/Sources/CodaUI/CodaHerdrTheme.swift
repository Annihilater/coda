import CodaKit
import SwiftUI
#if os(iOS)
import UIKit
#elseif os(macOS)
import AppKit
#endif

// MARK: - Herdr.dev Mobile & iPadOS Design Tokens

public enum CodaHerdrTheme {
    public static let canvasBG = Color(hex: 0x0B0C0F)
    public static let cardBG = Color(hex: 0x13151A)
    public static let elevatedBG = Color(hex: 0x1A1D24)
    public static let pillActiveBG = Color(hex: 0x232731)
    public static let borderHairline = Color(hex: 0x262A33)

    public static let textPrimary = Color(hex: 0xF0F3F6)
    public static let textSecondary = Color(hex: 0x8B949E)
    public static let textDim = Color(hex: 0x59626E)

    public static let amberNeedsYou = Color(hex: 0xF59E0B)
    public static let blueUnread = Color(hex: 0x3B82F6)
    public static let cyanWorking = Color(hex: 0x38BDF8)
    public static let greenDiff = Color(hex: 0x3FB950)
    public static let redDiff = Color(hex: 0xF85149)

    private static let avatarPalette: [Color] = [
        Color(hex: 0x2563EB), // Blue
        Color(hex: 0x059669), // Emerald
        Color(hex: 0x7C3AED), // Violet
        Color(hex: 0xD97706), // Amber
        Color(hex: 0x0891B2), // Cyan
        Color(hex: 0xDB2777), // Rose
        Color(hex: 0x4F46E5), // Indigo
        Color(hex: 0x0D9488), // Teal
    ]

    public static func workspaceColor(for name: String) -> Color {
        var hash: UInt64 = 5381
        for b in name.lowercased().utf8 {
            hash = ((hash &<< 5) &+ hash) &+ UInt64(b)
        }
        return avatarPalette[Int(hash % UInt64(avatarPalette.count))]
    }

    public static func initialLetter(for name: String) -> String {
        let trimmed = name.trimmingCharacters(in: .whitespacesAndNewlines)
        if let first = trimmed.first(where: { $0.isLetter || $0.isNumber }) {
            return String(first).uppercased()
        }
        return "C"
    }
}

public extension Color {
    init(hex: UInt32, alpha: Double = 1.0) {
        let r = Double((hex >> 16) & 0xFF) / 255.0
        let g = Double((hex >> 8) & 0xFF) / 255.0
        let b = Double(hex & 0xFF) / 255.0
        self.init(.sRGB, red: r, green: g, blue: b, opacity: alpha)
    }
}

// MARK: - Bilingual Helper (`EN` default + `中文`)

public func tr(_ en: String, _ zh: String, locale: CodaUILocale) -> String {
    locale == .zh ? zh : en
}

public func formatRelativeAge(since date: Date, now: Date = Date()) -> String {
    let elapsed = max(0, Int(now.timeIntervalSince(date)))
    if elapsed < 45 { return "now" }
    if elapsed < 3600 { return "\(elapsed / 60)m" }
    if elapsed < 86400 { return "\(elapsed / 3600)h" }
    return "\(elapsed / 86400)d"
}

// MARK: - Platform Utilities (Copy to Clipboard, Adaptive Modifiers)

public func copyTextToClipboard(_ text: String) {
    #if os(iOS)
    UIPasteboard.general.string = text
    #elseif os(macOS)
    NSPasteboard.general.clearContents()
    NSPasteboard.general.setString(text, forType: .string)
    #endif
}

public extension View {
    @ViewBuilder
    func codaNoAutocapAndSpelling() -> some View {
        #if os(iOS)
        self
            .textInputAutocapitalization(.never)
            .autocorrectionDisabled(true)
        #else
        self
            .autocorrectionDisabled(true)
        #endif
    }

    @ViewBuilder
    func codaDismissKeyboardInteractively() -> some View {
        #if os(iOS)
        self.scrollDismissesKeyboard(.interactively)
        #else
        self
        #endif
    }
}

// MARK: - Workspace Initial Avatar

public struct CodaWorkspaceAvatar: View {
    public let name: String
    public let size: CGFloat

    public init(name: String, size: CGFloat = 34) {
        self.name = name
        self.size = size
    }

    public var body: some View {
        let accent = CodaHerdrTheme.workspaceColor(for: name)
        ZStack {
            RoundedRectangle(cornerRadius: size * 0.25, style: .continuous)
                .fill(accent.opacity(0.20))
                .overlay(
                    RoundedRectangle(cornerRadius: size * 0.25, style: .continuous)
                        .strokeBorder(accent.opacity(0.45), lineWidth: 1)
                )
            Text(CodaHerdrTheme.initialLetter(for: name))
                .font(.system(size: size * 0.44, weight: .bold, design: .rounded))
                .foregroundStyle(accent)
        }
        .frame(width: size, height: size)
    }
}

// MARK: - Animated Cyan Working Spinner

public struct CodaWorkingSpinner: View {
    public init() {}

    public var body: some View {
        TimelineView(.animation(minimumInterval: 1.0 / 30.0, paused: false)) { context in
            let angle = context.date.timeIntervalSinceReferenceDate
                .truncatingRemainder(dividingBy: 0.9) / 0.9 * 360.0
            Circle()
                .trim(from: 0.15, to: 0.85)
                .stroke(
                    CodaHerdrTheme.cyanWorking,
                    style: StrokeStyle(lineWidth: 2, lineCap: .round)
                )
                .frame(width: 14, height: 14)
                .rotationEffect(.degrees(angle))
        }
    }
}

// MARK: - Herdr Status Indicator Badge

public struct CodaSessionStatusBadge: View {
    public let session: CodaAgentSession
    public let locale: CodaUILocale

    public init(session: CodaAgentSession, locale: CodaUILocale) {
        self.session = session
        self.locale = locale
    }

    public var body: some View {
        switch session.state {
        case .needsYou:
            Text(tr("Needs you", "待确认", locale: locale))
                .font(.system(size: 11, weight: .semibold))
                .foregroundStyle(CodaHerdrTheme.amberNeedsYou)
                .padding(.horizontal, 8)
                .padding(.vertical, 3.5)
                .background(
                    Capsule()
                        .fill(CodaHerdrTheme.amberNeedsYou.opacity(0.16))
                        .overlay(
                            Capsule()
                                .strokeBorder(CodaHerdrTheme.amberNeedsYou.opacity(0.42), lineWidth: 1)
                        )
                )
        case .error:
            Text(tr("Error", "异常", locale: locale))
                .font(.system(size: 11, weight: .semibold))
                .foregroundStyle(CodaHerdrTheme.redDiff)
                .padding(.horizontal, 8)
                .padding(.vertical, 3.5)
                .background(
                    Capsule()
                        .fill(CodaHerdrTheme.redDiff.opacity(0.16))
                        .overlay(
                            Capsule()
                                .strokeBorder(CodaHerdrTheme.redDiff.opacity(0.4), lineWidth: 1)
                        )
                )
        case .done:
            if session.unreadCompletion {
                Circle()
                    .fill(CodaHerdrTheme.blueUnread)
                    .frame(width: 8, height: 8)
                    .padding(.trailing, 2)
            } else {
                Text(formatRelativeAge(since: session.updatedAt))
                    .font(.system(size: 11, weight: .medium, design: .monospaced))
                    .foregroundStyle(CodaHerdrTheme.textDim)
            }
        case .working:
            CodaWorkingSpinner()
                .padding(.trailing, 2)
        case .idle:
            Text(formatRelativeAge(since: session.updatedAt))
                .font(.system(size: 11, weight: .medium, design: .monospaced))
                .foregroundStyle(CodaHerdrTheme.textDim)
        }
    }
}
