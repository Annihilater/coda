import Foundation

// MARK: - Smart-Path LAN Direct <-> Relay Backoff & Candidate Policy

public struct CodaDirectCandidate: Sendable, Equatable {
    public let host: String
    public let port: UInt16

    public init(host: String, port: UInt16) {
        self.host = host
        self.port = port
    }
}

/// Pure Sendable policy governing background LAN Direct upgrade probing while on Smart Relay:
/// - Exponential backoff (`15s -> 30s -> 60s -> 120s`) when LAN candidates are unreachable.
/// - Flap cooldown (`8s`) after a Direct P2P drop before re-attempting upgrade.
/// - Instant reset on network path transition (`NWPathMonitor`) or manual user probe.
public struct CodaSmartPathPolicy: Sendable {
    private var failuresByPeer: [UUID: Int] = [:]
    private var lastDropAtByPeer: [UUID: Date] = [:]
    public let baseInterval: TimeInterval
    public let maxInterval: TimeInterval
    public let dropCooldown: TimeInterval

    public init(
        baseInterval: TimeInterval = 15.0,
        maxInterval: TimeInterval = 120.0,
        dropCooldown: TimeInterval = 8.0
    ) {
        self.baseInterval = baseInterval
        self.maxInterval = maxInterval
        self.dropCooldown = dropCooldown
    }

    public mutating func recordUpgradeSuccess(peerId: UUID) {
        failuresByPeer[peerId] = 0
    }

    @discardableResult
    public mutating func recordProbeFailure(peerId: UUID) -> TimeInterval {
        let count = (failuresByPeer[peerId] ?? 0) + 1
        failuresByPeer[peerId] = count
        return backoffInterval(forFailures: count)
    }

    @discardableResult
    public mutating func recordDirectDrop(peerId: UUID, now: Date = Date()) -> TimeInterval {
        lastDropAtByPeer[peerId] = now
        failuresByPeer[peerId] = 1
        return max(baseInterval, dropCooldown)
    }

    public mutating func reset(peerId: UUID? = nil) {
        if let peerId {
            failuresByPeer.removeValue(forKey: peerId)
        } else {
            failuresByPeer.removeAll()
        }
    }

    public func canProbeNow(peerId: UUID, isDirect: Bool, now: Date = Date()) -> Bool {
        if isDirect { return false }
        guard let droppedAt = lastDropAtByPeer[peerId] else { return true }
        return now.timeIntervalSince(droppedAt) >= dropCooldown
    }

    public func nextDelay(for peerId: UUID) -> TimeInterval {
        let count = failuresByPeer[peerId] ?? 0
        if count <= 0 { return baseInterval }
        return backoffInterval(forFailures: count)
    }

    private func backoffInterval(forFailures count: Int) -> TimeInterval {
        let shift = min(max(count - 1, 0), 4)
        let mult = Double(1 << shift)
        return min(maxInterval, baseInterval * mult)
    }

    /// Deduplicates and ranks direct P2P candidates: UDP-discovered LAN peer first,
    /// followed by advertised `lan_ipv4`, then `cn_public_ipv6`.
    public static func buildDirectCandidates(
        udpFound: (host: String, port: UInt16)?,
        endpoints: [CodaEndpointCandidate]
    ) -> [CodaDirectCandidate] {
        var result: [CodaDirectCandidate] = []
        func appendUnique(_ host: String, _ port: UInt16) {
            let trimmed = host.trimmingCharacters(in: .whitespacesAndNewlines)
            guard !trimmed.isEmpty, port > 0 else { return }
            let cand = CodaDirectCandidate(host: trimmed, port: port)
            if !result.contains(cand) {
                result.append(cand)
            }
        }
        if let udpFound {
            appendUnique(udpFound.host, udpFound.port)
        }
        for ep in endpoints where ep.kind == "lan_ipv4" {
            if let parsed = parseHostPort(ep.addr) {
                appendUnique(parsed.0, parsed.1)
            }
        }
        for ep in endpoints where ep.kind == "cn_public_ipv6" {
            if let parsed = parseHostPort(ep.addr) {
                appendUnique(parsed.0, parsed.1)
            }
        }
        return result
    }

    public static func parseHostPort(_ addr: String) -> (String, UInt16)? {
        let trimmed = addr.trimmingCharacters(in: .whitespacesAndNewlines)
        if trimmed.hasPrefix("[") {
            guard let closeIdx = trimmed.firstIndex(of: "]") else { return nil }
            let host = String(trimmed[trimmed.index(after: trimmed.startIndex)..<closeIdx])
            let rest = trimmed[trimmed.index(after: closeIdx)...]
            guard rest.hasPrefix(":"), let port = UInt16(rest.dropFirst()) else { return nil }
            return (host, port)
        }
        guard let colonIdx = trimmed.lastIndex(of: ":") else { return nil }
        let host = String(trimmed[..<colonIdx])
        guard let port = UInt16(trimmed[trimmed.index(after: colonIdx)...]) else { return nil }
        return (host, port)
    }
}

// MARK: - Edge-Triggered OSC 7501 Native Notification Tracker

public enum CodaLocalAlertKind: String, Sendable, Equatable {
    case needsYou
    case error
    case done
}

public struct CodaLocalAlertPayload: Sendable, Equatable {
    public let sessionId: String
    public let kind: CodaLocalAlertKind
    public let title: String
    public let subtitle: String
    public let body: String

    public init(
        sessionId: String,
        kind: CodaLocalAlertKind,
        title: String,
        subtitle: String,
        body: String
    ) {
        self.sessionId = sessionId
        self.kind = kind
        self.title = title
        self.subtitle = subtitle
        self.body = body
    }
}

/// Pure Foundation edge-triggered tracker for OSC 7501 agent notifications (`needsYou`, `error`, `working -> done`).
public struct CodaNotificationTracker: Sendable {
    private var lastStateBySession: [String: HerdrLifecycleState] = [:]
    private var lastNotifiedAtBySession: [String: Date] = [:]
    public let cooldown: TimeInterval

    public init(cooldown: TimeInterval = 20.0) {
        self.cooldown = cooldown
    }

    public mutating func reset() {
        lastStateBySession.removeAll()
        lastNotifiedAtBySession.removeAll()
    }

    public mutating func observe(
        sessionId: String,
        machineName: String,
        workspaceName: String,
        taskTitle: String,
        statusMessage: String?,
        state: HerdrLifecycleState,
        activeSessionId: String?,
        locale: CodaUILocale,
        now: Date = Date()
    ) -> CodaLocalAlertPayload? {
        let prev = lastStateBySession[sessionId]
        lastStateBySession[sessionId] = state

        // Establish baseline on first sighting so startup / initial sync never spams alerts.
        guard let prev else { return nil }
        guard prev != state else { return nil }

        let kind: CodaLocalAlertKind?
        switch state {
        case .needsYou:
            kind = .needsYou
        case .error:
            kind = .error
        case .done where prev == .working:
            kind = .done
        default:
            kind = nil
        }
        guard let alertKind = kind else { return nil }

        // Suppress completion banners when the user is already viewing that exact terminal.
        if activeSessionId == sessionId && alertKind == .done {
            return nil
        }

        if let lastTs = lastNotifiedAtBySession[sessionId],
           now.timeIntervalSince(lastTs) < cooldown {
            return nil
        }
        lastNotifiedAtBySession[sessionId] = now

        let subtitle = machineName.isEmpty ? workspaceName : "\(workspaceName) · \(machineName)"
        let trimmedMsg = (statusMessage ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
        let trimmedTitle = taskTitle.trimmingCharacters(in: .whitespacesAndNewlines)
        let detail = !trimmedMsg.isEmpty ? trimmedMsg : trimmedTitle

        switch alertKind {
        case .needsYou:
            return CodaLocalAlertPayload(
                sessionId: sessionId,
                kind: .needsYou,
                title: locale == .zh ? "Coda · 需要交互审批" : "Coda · Action Needed",
                subtitle: subtitle,
                body: detail.isEmpty
                    ? (locale == .zh ? "AI Agent 正在等待你的确认或授权" : "AI Agent is waiting for your approval")
                    : detail
            )
        case .error:
            return CodaLocalAlertPayload(
                sessionId: sessionId,
                kind: .error,
                title: locale == .zh ? "Coda · 任务异常" : "Coda · Task Error",
                subtitle: subtitle,
                body: detail.isEmpty
                    ? (locale == .zh ? "终端任务返回异常状态" : "Terminal agent reported an error")
                    : detail
            )
        case .done:
            return CodaLocalAlertPayload(
                sessionId: sessionId,
                kind: .done,
                title: locale == .zh ? "Coda · 任务完成" : "Coda · Task Completed",
                subtitle: subtitle,
                body: detail.isEmpty
                    ? (locale == .zh ? "后台 Agent 已完成当前任务" : "Background agent finished its run")
                    : detail
            )
        }
    }
}
