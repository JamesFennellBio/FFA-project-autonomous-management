// swift-tools-version: 6.0
//
// The iOS app as an xtool SwiftPM project. Almost everything is the Rust app
// (app/src-tauri, built as a static library); this package only links it with
// Tauri's Swift runtime and the barcode-scanner plugin, which the Xcode
// project from `cargo tauri ios` would otherwise do.
// scripts/ios-xtool.sh copies those Swift packages into .deps/ first.

import PackageDescription

let rustLib = Context.packageDirectory + "/../../target/aarch64-apple-ios/release"

let package = Package(
    name: "FarmLab",
    platforms: [
        .iOS(.v15),
    ],
    products: [
        // xtool: exactly one library product, the app.
        .library(name: "FarmLab", targets: ["FarmLab"]),
    ],
    dependencies: [
        .package(path: ".deps/.tauri/tauri-api"),
        .package(path: ".deps/barcode-scanner"),
    ],
    targets: [
        .target(
            name: "FarmLab",
            dependencies: [
                .product(name: "Tauri", package: "tauri-api"),
                .product(name: "tauri-plugin-barcode-scanner", package: "barcode-scanner"),
            ],
            linkerSettings: [
                .unsafeFlags(["-L", rustLib, "-lfarmlab_app_lib"]),
                .linkedLibrary("c++"),
                .linkedLibrary("objc"),
                .linkedLibrary("resolv"),
                .linkedFramework("AVFoundation"),
                .linkedFramework("CoreGraphics"),
                .linkedFramework("Foundation"),
                .linkedFramework("Metal"),
                .linkedFramework("MetalKit"),
                .linkedFramework("QuartzCore"),
                .linkedFramework("Security"),
                .linkedFramework("SystemConfiguration"),
                .linkedFramework("UIKit"),
                .linkedFramework("WebKit"),
            ]
        ),
    ]
)
