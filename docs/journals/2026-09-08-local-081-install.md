# SottoASR 0.8.1 local follow-up

- **Version:** 1.0
- **Date:** 2026-09-08
- **Status:** Implemented

## Contents

1. Diagnosis and changes
2. Cleanup experiment
3. Verification
4. Installation and limits

## 1. Diagnosis and changes

The reported recording had cleanup enabled, a downloaded and loaded model, and
`no_changes` with no suggestion. Its three bare `um` tokens and one `uh` failed
the comma-only candidate guard, so inference never ran. The follow-up accepts
standalone lowercase hesitation tokens with or without a comma and distinguishes
guard skips from unchanged results. Quoted tokens, identifiers, capitalized names,
paragraph boundaries, input limits and original-output preservation remain tested.

Native UI inspection reproduced the Settings X failure before installation:
clicking its close button left the window unchanged despite a saved draft.
The installed Tauri SDK close listener calls `destroy()` after accepting a close,
but our capability only permitted `close()`. The new capability grants destruction
to Settings only. The official [Window API](https://v2.tauri.app/reference/javascript/api/namespacewindow/)
and [core permission reference](https://v2.tauri.app/reference/acl/core-permissions/#window)
were checked alongside the actual installed SDK source.

Settings also retained “Ready. Save to enable AI suggestions” after enable had
already saved. Successful activation saves and cancelling a ready draft now clear
that stale reminder. Failed saves and pending setup keep truthful state.

See the [three-pass follow-up specification](../specs/2026-09-08-cleanup-and-settings-followup.md)
and [Settings close audit](../audit/2026-09-08-settings-close-followup.md).

## 2. Cleanup experiment

The detector fix alone did not fully clean the reported sentence. A bounded
comparison tested the unchanged prompt and three variants on the same stock
LFM2.5 350M MLX model/runtime. The selected frozen occurrence prompt improved
safe useful edits on known regression cases from 31/40 to 37/40, with harmful
suggestions falling from 33 to 31 across 96 cases. Independent 16-case quality
was unchanged: 8 exact, 6/8 useful edit cases, and 5 harmful suggestions.

The integrated production Rust-candidate/JSON/MLX/reconstruction probe selected
only candidate 0 on the user's exact sentence: the first `um` was removed, while
two `um` tokens and `uh` remained. This is an unresolved model-quality limitation.
The earlier manual probe selected candidate 3 because its JSON key order differed;
the archive records that correction and the exact production order. No automatic
cleanup-quality claim is made, and automatic delivery still uses the ordinary text.

The user was asked about automatic application versus review. No response had
arrived during this follow-up; the existing History-suggestion behavior remains.
The user's enabled preference stays enabled. The [portable follow-up evidence](../../benchmarks/llm/model-study-2026-09-08/bare-hesitation-followup/README.md)
contains the frozen prompt, known and independent fixtures, results and production probe.

## 3. Verification

| Check | Result | Captured output |
|---|---|---|
| Rust default tests | 168 passed | `/tmp/sotto-081-cargo-test.txt` |
| Rust strict Clippy, all targets | Clean | `/tmp/sotto-081-cargo-clippy.txt` |
| Frontend full suite | 145 passed, 17 suites | `/tmp/sotto-followup-frontend-tests-verified.txt` |
| Svelte/TypeScript | 0 errors, 0 warnings | `/tmp/sotto-followup-frontend-check-sdk-final.txt` |
| Python protocol | 6 passed | `/tmp/experiments/sotto-bare-hesitation/python-protocol-tests.log` |
| Frozen/production prompt equivalence | Matched | Follow-up benchmark manifest |
| Production app build | Passed, Developer ID signed | `/tmp/sotto-081-production-build.txt` |
| Installed signature/source match | Passed | `/tmp/sotto-081-installation.json` |

The SDK regression initially failed because of test harness file loading/types,
then its expected IPC signature; those harness problems were corrected before
the final full passing run. No production workaround was added for a test failure.
No ASR backend or model changed, so the already completed ASR comparison was not rerun.

## 4. Installation and limits

Installed 0.8.1 at `/Applications/SottoASR.app` at 12:03:34 UTC. The previous 0.8.0
bundle is retained at `~/Library/Application Support/com.sottoasr.app/app-backups/0.8.0-20260908T120322Z/SottoASR.app`.
The signed incoming bundle was copied and verified before quitting the idle app
through its normal guarded Cmd+Q action. The executable SHA-256 is
`9f73670c64f108396ef43d7f505f7f577f388d6df613bfa92bca58e9d1abc69f`.

Version, LSUIElement, strict signature, matching designated requirement and
installed/source sidecar equality were verified. Settings and history were
byte-identical at installation; settings remained unchanged after startup.
History subsequently changed normally when a real 6.112-second recording completed
at 12:06:40 UTC. Its status is `suggested`, with 184 ms inference and a saved
suggestion, confirming the installed live cleanup path without reading its text.
Startup also confirmed Accessibility functional checks, configured hotkeys and ASR readiness.

Native post-install X testing remains unverified: the computer-use tool could
inspect the existing Settings window before quit, but timed out on the running
menu-bar-only app after relaunch and could not reopen Settings via the synthetic
global shortcut. The actual SDK callback and clean/dirty/failed-save cases pass
automated tests. No macOS privacy permission was reset or granted to work around this limit.
The local build is signed but not notarized or publicly released. No commit,
push, tag, model-cache relocation or history deletion was performed.
