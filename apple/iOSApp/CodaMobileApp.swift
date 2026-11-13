import CodaUI
import SwiftUI

@main
struct CodaMobileApp: App {
    var body: some Scene {
        WindowGroup("Coda") {
            CodaAdaptiveRootView()
                .preferredColorScheme(.dark)
        }
    }
}
