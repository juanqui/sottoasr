// swift-tools-version:5.10
import PackageDescription

let package = Package(
    name: "probe",
    platforms: [.macOS(.v14)],
    dependencies: [
        .package(url: "https://github.com/FluidInference/FluidAudio.git", exact: "0.18.3")
    ],
    targets: [.executableTarget(
        name: "probe",
        dependencies: [.product(name: "FluidAudio", package: "FluidAudio")]
    )]
)
