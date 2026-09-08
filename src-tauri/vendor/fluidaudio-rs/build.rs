use std::path::PathBuf;
use std::process::Command;

fn main() {
    assert_eq!(
        std::env::var("CARGO_CFG_TARGET_OS").as_deref(),
        Ok("macos"),
        "FluidAudio is only supported on macOS"
    );
    println!("cargo:rerun-if-changed=swift/");
    println!("cargo:rerun-if-changed=Package.swift");
    println!("cargo:rerun-if-changed=Package.resolved");

    let manifest = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let build = PathBuf::from(std::env::var_os("OUT_DIR").unwrap()).join("swift-build");
    let target = std::env::var("CARGO_CFG_TARGET_ARCH").unwrap();
    let arch = match target.as_str() {
        "aarch64" => "arm64",
        "x86_64" => "x86_64",
        _ => panic!("Unsupported architecture"),
    };
    let status = Command::new("swift")
        .args(["build", "-c", "release", "--arch", arch, "--build-path"])
        .arg(&build)
        .current_dir(&manifest)
        .status()
        .expect("Failed to run swift build");
    assert!(status.success(), "FluidAudio Swift bridge build failed");

    println!(
        "cargo:rustc-link-search=native={}",
        build.join("release").display()
    );
    println!("cargo:rustc-link-lib=static=FluidAudioBridge");
    // SwiftPM's static product references its binary dependency rather than
    // embedding it. Link that checksum-pinned framework's static archive too.
    let artifacts = build
        .join("artifacts")
        .join("fluidaudio")
        .join("NemoTextProcessing")
        .join("NemoTextProcessing.xcframework")
        .join("macos-arm64_x86_64");
    println!("cargo:rustc-link-search=native={}", artifacts.display());
    println!("cargo:rustc-link-lib=static=text_processing_rs");
    for framework in [
        "Foundation",
        "AVFoundation",
        "CoreML",
        "Accelerate",
        "Metal",
        "MetalPerformanceShaders",
    ] {
        println!("cargo:rustc-link-lib=framework={framework}");
    }
    println!("cargo:rustc-link-lib=dylib=swiftCore");
    println!("cargo:rustc-link-lib=c++");
}
