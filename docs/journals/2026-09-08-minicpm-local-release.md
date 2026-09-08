# MiniCPM automatic cleanup and warmup release

- **Version:** 1.0
- **Date:** 2026-09-08
- **Status:** In Review

## Contents

1. Behavior and model choice
2. Local verification
3. Installation
4. GitHub build and release

## 1. Behavior and model choice

The old pipeline saved a suggestion while pasting the original. The new pipeline
uses validated cleanup for paste/copy and saved text, and retains original ASR in
History. MiniCPM proposes complete text; Rust accepts only supported source
removals, with vocabulary/dictionary/quote/code/identifier protections. Invalid,
incomplete or unsupported edits retain the complete source. Cleanup stays off
by default; the user's existing enabled preference is preserved.

The user chose MiniCPM after the [head-to-head study](../../benchmarks/llm/model-study-2026-09-08/direct-cleanup-diagnostic/HEAD-TO-HEAD.md).
The selected model is official OpenBMB MiniCPM5-2B MLX 4-bit at revision
`32f8dd5df1188512a20413f1297083238306634c`, using the frozen D7 prompt and v6
source validator. Pollard's larger mixed quantization gave no aggregate cleanup
improvement. Earlier independent qualification failed; the following development
smoke does not prove universal semantic preservation or replace qualification.

Enabled startup and explicit preparation run one fixed synthetic warmup before
reporting readiness. Subsequent loads reuse the warmed resident model. Warmup
never uses private transcript data or retains a request cache. Disabled cleanup
starts no warmup. Shared setup also handles migration when a saved enabled
preference refers to the previous model. Setup failures remain visible in Settings.

## 2. Local verification

The initial 0.8.2 build passed 177 Rust tests, 145 frontend tests, frontend type
checks, 16 Python protocol tests and strict Clippy. The final validator tests
also cover 672 frozen Python-oracle cases plus Unicode, dictionary overlap and
resource limits. Review found and fixed a port-only overlapping dictionary match
and removed the unused legacy policy from production compilation; its historical
benchmark and regression tests remain available.

The signed 0.8.2 bundled sidecar passed all 15 smoke cases: 13 exact targets,
with two remaining edits falling back to complete source. Nine outputs equaled
source, including intentionally unchanged targets and one language abstention.
The user's exact sentence produced:

> This is a test to see if this can remove all those things from the sentences.

Wire latency was 1.202 seconds; the longest smoke request was 3.107 seconds.
These include neither microphone capture nor ASR/paste latency. The actual
model snapshot's seven required files passed full SHA-256 verification; runtime
versions matched Python 3.14.5, MLX 0.32.2, mlx-lm 0.31.3, Transformers 5.3.0
and huggingface-hub 1.7.2. Source and packaged sidecar matched byte-for-byte.

The 0.8.3 dependency/warmup change passes all 179 Rust tests and 19 Python
protocol tests. The frontend code is unchanged from the passing 145-test/type
check run. Workflow syntax passes actionlint 1.7.12 and all configuration
assertions pass. The final Developer ID signed app and DMG built successfully. The packaged
0.8.3 smoke passed 15/15 cases: cold load plus synthetic warmup took 2.950 s,
repeat load took 0.066 ms without repeating inference, and the first user
request took 0.988 s. Three release-target guard tests also pass.
Reproduction lives in [release-smoke](../../benchmarks/llm/release-smoke/README.md).
No ANE or energy measurement is claimed for this MLX cleanup model.

## 3. Installation

Version 0.8.2 was built and verified, but its installation stopped at a script
comparison that included different executable paths in otherwise identical
code-signing requirements. The comparison was corrected to use only the actual
designated requirement. No installed bundle changed during that attempt. The
user then requested warmup and the next patch, so installation targets 0.8.3.

Version 0.8.3 was installed and launched from `/Applications/SottoASR.app` at
15:52 UTC. Strict signature verification passed, designated identity was unchanged,
and settings/history hashes were preserved. The previous 0.8.1 bundle was backed
up under the application support directory. Startup logs confirm ASR ready at
15:52:05 and the enabled MiniCPM model preloaded, warmed and ready at 15:52:09.
Accessibility is granted, but its functional probe reported -25212; actual
paste after installation remains unverified.

Native Settings close verification remains limited by computer-use timeouts on
the menu-bar-only app. The real Tauri SDK close callback/capability and unsaved
settings behavior pass automated tests. No Accessibility permission was reset.

## 4. GitHub build and release

Previously only version tags and manual dispatch triggered the signed release
workflow. The PR adds full CoreML/backend/frontend/Python checks and ad-hoc macOS
packaging for pull requests, without signing secrets. Main pushes and version
tags trigger a signed draft release. Manual dispatch defaults to signed build
verification and artifact upload without creating or publishing a release.
The exact commit is supplied as the release target.

The initial current dependency audit failed on crossbeam-epoch, h2, quick-xml
and rkyv. Compatible parent/patch updates resolved every vulnerability:
`cargo-audit 0.22.2` reports zero vulnerabilities across 696 packages, with no
advisory ignores (RustSec database `bf25f6575a93a35f30796c65c0ed91bee7fa19fd`).
Ten informational dependency warnings remain; these are not a clean bill of
health for every transitive dependency. Release guards prevent reusing a version
tag or draft for a different source commit. Repository signing/notarization/updater secret names exist;
only the actual GitHub run can verify their current usability. Local signing
alone does not verify Apple's remote notarization service or GitHub credentials.
