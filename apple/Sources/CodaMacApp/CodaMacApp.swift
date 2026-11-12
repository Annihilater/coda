import CodaUI
import SwiftUI

@main
struct CodaMacApp: App {
    var body: some Scene {
        WindowGroup("Coda · Agent Mesh") {
            CodaAdaptiveRootView()
                .frame(minWidth: 980, minHeight: 640)
        }

        #if os(macOS)
        MenuBarExtra("Coda", systemImage: "point.3.connected.trianglepath.dotted") {
            VStack(alignment: .leading, spacing: 8) {
                Text("Coda Host Daemon · Active")
                    .font(.headline)
                Text("Herdr Core v0.1.20 · Zero-Knowledge E2EE")
                    .font(.caption)
                    .foregroundStyle(.secondary)
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
