use std::path::{Path, PathBuf};
use std::process::Command;

fn apply_pinned_patch(checkout: &Path, source: &str, patch: &Path, original: &str, patched: &str) {
    let digest = Command::new("shasum")
        .args(["-a", "256"])
        .arg(checkout.join(source))
        .output()
        .expect("Could not verify pinned FluidAudio source");
    assert!(
        digest.status.success(),
        "Could not hash pinned FluidAudio source"
    );
    let digest = String::from_utf8(digest.stdout).expect("Invalid source checksum");
    match digest.split_whitespace().next().unwrap_or("") {
        value if value == original => {
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
        }
        value if value == patched => (),
        _ => panic!(
            "FluidAudio source changed; review {} before building",
            patch.display()
        ),
    }
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
    println!("cargo:rerun-if-changed=patches/alignment-stack.patch");
    println!("cargo:rerun-if-changed=patches/ctc-tensor-access.patch");

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
    apply_pinned_patch(&checkout,
        "Sources/FluidAudio/ASR/Parakeet/SlidingWindow/CustomVocabulary/Rescorer/VocabularyRescorer+Utilities.swift",
        &manifest.join("patches/alignment-stack.patch"),
        "9fcef3028b4457637a0fea5c2787ddaadc2ca9cba62ebeef1bef33422060ad95",
        "7d42688ee320561397b17c735d246e0600f508d486c11781178de5e15b19118f");
    apply_pinned_patch(&checkout,
        "Sources/FluidAudio/ASR/Parakeet/SlidingWindow/CustomVocabulary/WordSpotting/CtcKeywordSpotter+Inference.swift",
        &manifest.join("patches/ctc-tensor-access.patch"),
        "0384a92498312bc7dc842a0158130287c5423a5f9f51220878f47d64a51314a0",
        "ba8a620a88bd32ca617d77282f65478a6ea6a5758876ebd8884a1033b3dd2123");
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
