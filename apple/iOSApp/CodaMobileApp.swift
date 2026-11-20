import CodaUI
import SwiftUI

@main
struct CodaMobileApp: App {
    @StateObject private var store = CodaWorkspaceStore()

    var body: some Scene {
        WindowGroup("Coda") {
            CodaAdaptiveRootView(store: store)
                .preferredColorScheme(.dark)
        }
    }
}
