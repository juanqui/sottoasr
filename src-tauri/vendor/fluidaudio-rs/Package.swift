// swift-tools-version:5.10
import PackageDescription

let package = Package(
    name: "FluidAudioBridge",
    platforms: [.macOS(.v14)],
    products: [.library(name: "FluidAudioBridge", type: .static, targets: ["FluidAudioBridge"])],
    dependencies: [
        .package(url: "https://github.com/FluidInference/FluidAudio.git", exact: "0.15.6")
    ],
    targets: [.target(
        name: "FluidAudioBridge",
        dependencies: [.product(name: "FluidAudio", package: "FluidAudio")],
        path: "swift"
    )]
)
