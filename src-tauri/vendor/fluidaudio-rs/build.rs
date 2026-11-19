use std::path::{Path, PathBuf};
use std::process::Command;

fn sha256(path: &Path) -> String {
    let digest = Command::new("shasum")
        .args(["-a", "256"])
        .arg(path)
        .output()
        .expect("Could not verify pinned FluidAudio source");
    assert!(
        digest.status.success(),
        "Could not hash pinned FluidAudio source {}",
        path.display()
    );
    String::from_utf8(digest.stdout)
        .expect("Invalid source checksum")
        .split_whitespace()
        .next()
        .unwrap_or("")
        .to_owned()
}

/// Applies one reviewed patch to one SDK source. Any other upstream content,
/// before or after applying, stops the build.
fn apply_pinned_patch(checkout: &Path, source: &str, patch: &Path, original: &str, patched: &str) {
    let file = checkout.join(source);
    let digest = sha256(&file);
    if digest == patched {
        return;
    }
    assert!(
        digest == original,
        "FluidAudio source changed; review {} before building",
        patch.display()
    );
    let applied = Command::new("git")
        .arg("apply")
        .arg(patch)
        .current_dir(checkout)
        .status()
        .expect("Could not apply FluidAudio patch");
    assert!(
        applied.success(),
        "FluidAudio patch failed: {}",
        patch.display()
    );
    assert_eq!(
        sha256(&file),
        patched,
        "FluidAudio patch {} produced unexpected source",
        patch.display()
    );
}

fn main() {
    assert_eq!(
        std::env::var("CARGO_CFG_TARGET_OS").as_deref(),
        Ok("macos"),
        "FluidAudio is only supported on macOS"
    );
    println!("cargo:rerun-if-changed=swift/");
    println!("cargo:rerun-if-changed=Package.swift");
    println!("cargo:rerun-if-changed=Package.resolved");
    println!("cargo:rerun-if-changed=patches/model-registry-revision.patch");
    println!("cargo:rerun-if-changed=patches/tree-lister-revision.patch");

    let manifest = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let build = PathBuf::from(std::env::var_os("OUT_DIR").unwrap()).join("swift-build");
    let target = std::env::var("CARGO_CFG_TARGET_ARCH").unwrap();
    let arch = match target.as_str() {
        "aarch64" => "arm64",
        "x86_64" => "x86_64",
        _ => panic!("Unsupported architecture"),
    };
    let resolved = Command::new("swift")
        .args(["package", "--build-path"])
        .arg(&build)
        .args(["resolve", "--force-resolved-versions"])
        .current_dir(&manifest)
        .status()
        .expect("Failed to resolve pinned Swift dependencies");
    assert!(
        resolved.success(),
        "Pinned Swift dependency resolution failed"
    );
    let checkout = build.join("checkouts/FluidAudio");
    // Model downloads read the commits recorded in README.md, never `main`.
    apply_pinned_patch(&checkout,
        "Sources/FluidAudio/ModelRegistry.swift",
        &manifest.join("patches/model-registry-revision.patch"),
        "8d8d217cd42b79c943f2df5831cf11323533dbbe4032550e50c3ee328e696edd",
        "3c280a4c459bd2dad605f8a0e0c38320f0caa2caf6216a8eb1f96a2d8b7c3e99");
    apply_pinned_patch(&checkout,
        "Sources/FluidAudio/Shared/Download/HFTreeLister.swift",
        &manifest.join("patches/tree-lister-revision.patch"),
        "0ac831657c565eb62184169beeb5d0e4fe8cf4c472478b4f9deb37ac1f924a49",
        "ecfb8cbf62dd1b67d8cee76efa117a8d10221e6e06e50b3e34e51a1446fe27f7");
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
        "CryptoKit",
    ] {
        println!("cargo:rustc-link-lib=framework={framework}");
    }
    println!("cargo:rustc-link-lib=dylib=swiftCore");
    println!("cargo:rustc-link-lib=c++");
}
