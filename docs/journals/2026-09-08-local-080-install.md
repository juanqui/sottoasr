# SottoASR 0.8.0 Local macOS Installation

- **Version:** 1.1
- **Date:** 2026-09-08
- **Status:** Implemented

The user requested the next minor version and installation on this Mac for testing. This follows the [dictation reliability implementation](../specs/2026-09-07-dictation-reliability.md). Scope is a signed local build and installation; no git commit, push, tag, or public release is authorized.

## Tasks

- [x] Bump the five version files to 0.8.0, update the changelog and website badge, and verify consistency.
- [x] Run the automated pre-release checks and build the production app with the existing Developer ID identity.
- [x] Verify bundle version, signature, minimum macOS, menu-bar-only configuration, and bundled production sidecar.
- [x] Quit the idle installed app, preserve the previous bundle, install the new bundle at the same path, and launch through Launch Services.
- [x] Confirm the running installed version and startup health; stage metadata and document any manual testing needed.

## Installation record

The automated pre-release script completed with **10 passed, 0 failed**, including 103 Rust tests and clean build/lint/type checks. Output: `/tmp/sotto-080-pre-release.txt`; component logs: `/tmp/sotto-smoke-{build,clippy,check,test}.txt`. The optimized production build passed in 2m 19s and was signed with the existing Developer ID identity. Build output: `/tmp/sotto-080-production-build.txt`.

Previous installation: `/Applications/SottoASR.app`, version 0.7.6, signed by Developer ID Application: Juan Villa (DR3FNR9MW9). The same valid signing identity is available locally. Settings, dictation history, and model caches remain in their current locations.


Installed **0.8.0** at `/Applications/SottoASR.app` and launched it through Launch Services. The previous bundle is preserved at `/Users/juanqui/Library/Application Support/com.sottoasr.app/app-backups/0.7.6-20260908T003857/SottoASR.app`. The installer verified idle state from recording completion and overlay-hide events, stopped the existing process with SIGTERM, moved the old bundle to backup, and placed the verified new bundle at the existing path. Native UI inspection was unavailable; no Accessibility permission was granted to the installer or reset for Sotto.

Bundle verification passed: version 0.8.0, macOS minimum 14.0, `LSUIElement=true`, matching designated signing requirement, correct production sidecar, and only system dynamic dependencies. The installed executable matches the production build exactly; `codesign --verify --deep --strict` passes. This is a locally signed test installation, not a notarized or published release. Evidence: `/tmp/sotto-080-bundle-verification.txt` and `/tmp/sotto-080-install.txt`.

The running process was verified at the installed path with Accessory activation policy. Fresh startup logs at 05:38:57–05:39:13 UTC confirmed Accessibility granted and functional check passed, hotkeys registered, FluidAudio 0.15.6 ready with Parakeet TDT v3, and ASR engine ready. There were no startup warning/error markers. Settings and history remained byte-identical through installation and startup; AI cleanup stayed disabled. No model cache was moved or deleted.

The application is ready for the user's microphone and paste testing. Existing hotkeys remain F16 for push-to-talk, F13 for toggle, and Command+Shift+Comma for Settings. In this initial build, optional stock cleanup required a separate Settings download; the subsequent installation below replaces that flow. Version/changelog/badge updates and this journal are staged with the previously implemented changes; no commit, push, tag, or publication was performed.


## Overnight revision installed

After the user's subsequent vocabulary/settings/performance request, completed
[five sequential application reviews](../audit/2026-09-08-five-pass-review.md) and
rebuilt the same unreleased local minor version0.8.0. The signed production build
passed in31.84seconds (plus bundling), including the final frontend assets and
bundled sidecar. Existing bundle-identifier advice and unavailable notarization
credentials are the only Tauri packaging notices; changing the established
identifier would disrupt preferences and TCC, so it is preserved.

Installed at03:30CDT /08:30UTC on September8,2026. The previous0.8.0 test bundle is
preserved at `/Users/juanqui/Library/Application Support/com.sottoasr.app/app-backups/0.8.0-20260908T083010Z/SottoASR.app`. The installer copied and verified a
staged bundle before stopping the idle existing process, backed up the old bundle,
atomically renamed the new bundle into `/Applications/SottoASR.app`, and launched
through Launch Services. No cache was moved or deleted. The earlier0.7.6 backup
also remains available.

Installed executable SHA256: `7f64d99a974500fbfc01c65c273ffc1b5da8cbfd056fb3f1627b8527128a61c3`.
Deep strict signature verification passes with the same Developer ID and designated
requirement. Bundle version/build are0.8.0, minimum macOS14, LSUIElement=true.
The bundled sidecar matches the checked-in source. The running installed process
is96186 with Accessory activation policy1 and completed launch. Startup logs at
08:30:11–08:30:12UTC confirm granted/functional Accessibility, original F16/F13
and Command+Shift+Comma shortcuts, FluidAudio0.15.6/ParakeetTDTv3 ready, and no
startup warning/error. Both settings and history SHA256 remain identical before
and after replacement/startup; cleanup remains false.

Final checks:164 default-backend Rust tests,162 no-ASR Rust tests,135 frontend
tests,6 Python protocol tests, clean Clippy/all-targets and Svelte/TypeScript,
production build/CSP browser checks,16/16 long quiet-tail ASR fixtures,7 vocabulary
cache fault cases from review2, and3 non-destructive ASR cache fault cases plus a
real isolated-clone smoke from review5. The first release-check script returned8/10;
its lint and optional-feature failures were fixed and those checks rerun successfully,
without repeating already-passing checks. The audit records exact logs and review
fingerprints.

The Settings switch now prepares optional cleanup automatically. Expanded model
validation rejected automatic cleanup, so results are experimental History
suggestions requiring explicit Copy suggestion; ordinary output remains recognized
text plus dictionary corrections. Vocabulary words use an independent acoustic
model prepared when saved; ambiguous homophones and versioned identifiers retain
stated limits. Physical microphone unplug/lock/wake/paste behavior and battery watts
are not certified by synthetic/browser tests. This remains a signed local test
installation, without notarization, a commit, push, tag or public release.

Evidence: `/tmp/sotto-080-overnight-{production-build,installation.json,startup.txt}`,
`/tmp/sotto-080-overnight-{pre-release,clippy-final,fallback-tests,frontend-tests,python-tests,asr-tail}.txt`.


The optional ONNX backend also passes its locked Cargo check after removing one
unused local confined to `#[cfg(feature = "asr-parakeet")]`. The installed default
FluidAudio bundle excludes that code and remains unchanged. Final repository
fingerprint and the matching installed-build fingerprint are recorded in the audit.
All changes are staged for review; no commit or push was made.
