// swift-tools-version: 6.0
import PackageDescription

let package = Package(
    name: "CodaApple",
    platforms: [
        .iOS(.v17),
        .macOS(.v14),
    ],
    products: [
        .library(name: "CodaKit", targets: ["CodaKit"]),
        .library(name: "CodaGhosttyKit", targets: ["CodaGhosttyKit"]),
        .library(name: "CodaUI", targets: ["CodaUI"]),
        .executable(name: "CodaMacApp", targets: ["CodaMacApp"]),
        .executable(name: "CodaSelfTest", targets: ["CodaSelfTest"]),
    ],
    targets: [
        .target(
            name: "CodaKit",
            path: "Sources/CodaKit"
        ),
        .target(
            name: "CodaGhosttyKit",
            dependencies: ["CodaKit"],
            path: "Sources/CodaGhosttyKit"
        ),
        .target(
            name: "CodaUI",
            dependencies: ["CodaKit", "CodaGhosttyKit"],
            path: "Sources/CodaUI"
        ),
        .executableTarget(
            name: "CodaMacApp",
            dependencies: ["CodaKit", "CodaGhosttyKit", "CodaUI"],
            path: "Sources/CodaMacApp"
        ),
        .executableTarget(
            name: "CodaSelfTest",
            dependencies: ["CodaKit", "CodaGhosttyKit"],
            path: "Sources/CodaSelfTest"
        ),
        .testTarget(
            name: "CodaKitTests",
            dependencies: ["CodaKit", "CodaGhosttyKit"],
            path: "Tests/CodaKitTests"
        ),
    ]
)
