// swift-tools-version:5.9
// The phone side of the chrome-use mobile remote, kept free of UIKit so the
// protocol, crypto and connection layer run under `swift test` on macOS.
import PackageDescription

let package = Package(
    name: "ChromeUseKit",
    platforms: [.iOS(.v17), .macOS(.v14)],
    products: [
        .library(name: "ChromeUseKit", targets: ["ChromeUseKit"]),
    ],
    targets: [
        .target(name: "ChromeUseKit"),
        .testTarget(
            name: "ChromeUseKitTests",
            dependencies: ["ChromeUseKit"],
            resources: [.copy("Fixtures")]
        ),
    ]
)
