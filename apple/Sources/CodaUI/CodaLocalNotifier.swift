import CodaKit
import Foundation
#if canImport(UserNotifications)
import UserNotifications
#endif
#if os(iOS)
import UIKit
#endif

// MARK: - Subtle iOS Tactile Haptics (no-op on macOS)

@MainActor
public enum CodaHaptics {
    public static func lightTap() {
        #if os(iOS)
        let gen = UIImpactFeedbackGenerator(style: .light)
        gen.impactOccurred()
        #endif
    }

    public static func mediumTap() {
        #if os(iOS)
        let gen = UIImpactFeedbackGenerator(style: .medium)
        gen.impactOccurred()
        #endif
    }

    public static func success() {
        #if os(iOS)
        let gen = UINotificationFeedbackGenerator()
        gen.notificationOccurred(.success)
        #endif
    }
}

// MARK: - Native Local Notification Adapter + Tap-to-Jump Delegate

/// Native iOS / iPadOS / macOS local notification adapter backed by `UNUserNotificationCenter`.
/// Safely checks for a real `.app` bundle before touching `UNUserNotificationCenter.current()`
/// so SwiftPM test harnesses and CLI binaries never raise an unbundled Obj-C exception.
@MainActor
public final class CodaLocalNotifier: NSObject {
    public static let shared = CodaLocalNotifier()

    private var onSelectSession: (@MainActor (String) -> Void)?
    private var delegateInstalled = false

    private override init() {
        super.init()
    }

    public var isAvailableInCurrentProcess: Bool {
        #if canImport(UserNotifications)
        return Bundle.main.bundleURL.pathExtension == "app" && Bundle.main.bundleIdentifier != nil
        #else
        return false
        #endif
    }

    /// Registers a handler invoked when the user taps an OSC 7501 notification banner,
    /// jumping straight to the alerting terminal session.
    public func registerSessionTapHandler(_ handler: @escaping @MainActor (String) -> Void) {
        self.onSelectSession = handler
        #if canImport(UserNotifications)
        guard isAvailableInCurrentProcess, !delegateInstalled else { return }
        UNUserNotificationCenter.current().delegate = self
        delegateInstalled = true
        #endif
    }

    public func requestAuthorization() async -> Bool {
        #if canImport(UserNotifications)
        guard isAvailableInCurrentProcess else { return false }
        let center = UNUserNotificationCenter.current()
        if !delegateInstalled {
            center.delegate = self
            delegateInstalled = true
        }
        do {
            return try await center.requestAuthorization(options: [.alert, .sound, .badge])
        } catch {
            return false
        }
        #else
        return false
        #endif
    }

    public func deliver(_ payload: CodaLocalAlertPayload) {
        #if canImport(UserNotifications)
        guard isAvailableInCurrentProcess else { return }
        let center = UNUserNotificationCenter.current()
        let content = UNMutableNotificationContent()
        content.title = payload.title
        content.subtitle = payload.subtitle
        content.body = payload.body
        content.userInfo = ["sessionId": payload.sessionId, "kind": payload.kind.rawValue]
        if payload.kind == .needsYou || payload.kind == .error {
            content.sound = .default
        }
        let identifier = "coda.osc7501.\(payload.sessionId)"
        let request = UNNotificationRequest(identifier: identifier, content: content, trigger: nil)
        center.add(request, withCompletionHandler: nil)
        #else
        _ = payload
        #endif
    }

    fileprivate func dispatchTappedSession(_ sessionId: String) {
        onSelectSession?(sessionId)
    }
}

#if canImport(UserNotifications)
extension CodaLocalNotifier: UNUserNotificationCenterDelegate {
    public nonisolated func userNotificationCenter(
        _ center: UNUserNotificationCenter,
        willPresent notification: UNNotification,
        withCompletionHandler completionHandler: @escaping (UNNotificationPresentationOptions) -> Void
    ) {
        completionHandler([.banner, .sound, .list])
    }

    public nonisolated func userNotificationCenter(
        _ center: UNUserNotificationCenter,
        didReceive response: UNNotificationResponse,
        withCompletionHandler completionHandler: @escaping () -> Void
    ) {
        let sid = response.notification.request.content.userInfo["sessionId"] as? String
        if let sid, !sid.isEmpty {
            Task { @MainActor in
                CodaLocalNotifier.shared.dispatchTappedSession(sid)
            }
        }
        completionHandler()
    }
}
#endif
