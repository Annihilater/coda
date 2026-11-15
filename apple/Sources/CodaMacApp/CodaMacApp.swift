import CodaUI
import SwiftUI

@main
struct CodaMacApp: App {
    @StateObject private var store = CodaWorkspaceStore()

    var body: some Scene {
        WindowGroup("Coda · Agent Mesh") {
            CodaAdaptiveRootView(store: store)
                .frame(minWidth: 980, minHeight: 640)
        }

        #if os(macOS)
        MenuBarExtra("Coda", systemImage: "point.3.connected.trianglepath.dotted") {
            VStack(alignment: .leading, spacing: 8) {
                Text(store.isSignedIn ? "Coda · @\(store.settings.ownerHandle)" : "Coda · Signed Out")
                    .font(.headline)
                Text(
                    store.isSignedIn
                        ? "\(store.onlineMachineCount) hosts online · \(store.sessions.count) sessions · \(store.attentionBadgeCount) attention"
                        : "Herdr Core · Zero-Knowledge E2EE"
                )
                .font(.caption)
                .foregroundStyle(.secondary)
                if store.isSignedIn {
                    Button("Sync Mesh Now") {
                        Task { await store.refreshMachinesAndSessions(forceWorkspaces: true) }
                    }
                }
                Divider()
                Button("Quit Coda") {
                    NSApplication.shared.terminate(nil)
                }
            }
            .padding(10)
        }
        #endif
    }
}
