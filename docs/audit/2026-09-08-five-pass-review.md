# Five Sequential Application Reviews

- **Version:** 1.0
- **Date:** 2026-09-08
- **Status:** Implemented

## Table of Contents

1. [Method](#1-method)
2. [Review ledger](#2-review-ledger)
3. [Evidence and limits](#3-evidence-and-limits)
4. [Pass 1](#4-pass-1--integrated-failure-paths)
5. [Pass 2](#5-pass-2--independent-interaction-review)
6. [Pass 3](#6-pass-3--revised-application-review)
7. [Pass 4](#7-pass-4--independent-runtime-and-interaction-review)
8. [Pass 5](#8-pass-5--final-integrated-application-review)

## 1. Method

This audit implements the five application reviews requested in the
[approved specification](../specs/2026-09-08-vocabulary-settings-performance.md).
The three specification reviews and initial specialist research are not counted.
Each application review follows the prior review's implemented and checked fixes.
Source fingerprints include application code, sidecar, bridge, and build configuration;
temporary experiments and prose changes do not substitute for an application fix.

Each pass covers audio/ASR, cleanup/vocabulary, settings/UI/history,
persistence/clipboard, startup/exit/updater/packaging, privacy, and accessibility.
Findings distinguish demonstrated failures, source-proven defects, and untested
physical/OS behavior. Focused regression checks run after corrections; consolidated
builds and device checks follow the final review.

## 2. Review ledger

| Pass | Reviewed source | Coverage and findings | Implemented improvement | Verification | State |
| --- | --- | --- | --- | --- | --- |
| 1 | `5d20037d470c` → `87d68cd7fc88`, 06:28–06:42 UTC | Ownership, capture, tray, shortcuts, UI/lifecycle | Implemented below | 127 Rust; 109 initial + 8 focused frontend; clean Svelte; visual checks | Complete |
| 2 | `87d68cd7fc88` → `bcd17fc75293`, 06:42–07:10 UTC | Independent whole-app interaction and failure review | Cache repair, recording generations/recovery, owned process shutdown, native exports and links | 138 Rust; 49 focused + 4 error-race frontend; clean Svelte; error-card visuals | Complete |
| 3 | `bcd17fc75293` → `49808cea8316`, 07:10–07:34 UTC | Independent whole-app audio, recovery and OS interaction review | Interrupted-capture recovery, native error focus, safe activation, async clipboard, preference guards, Reduce Motion | 146 Rust; 10 focused frontend; clean Svelte; portable UI smoke | Complete |
| 4 | `49808cea8316` → `8eef57195ea6`, 07:34–08:07 UTC | Independent whole-app runtime and interaction review | Replayable overlay, durable history acknowledgements, capture-safe shortcuts/exit, bounded stdin and permission recovery | 158 Rust; 27 focused frontend; clean Svelte; production CSP/visual checks | Complete |
| 5 | `8eef57195ea6` → `d62fcb957c2d`, 08:07–08:26 UTC | Independent whole-app final review and revised-output containment | Reviewed suggestions, blocking capture finish, non-destructive cache loading, recovery/CSV/eviction fixes | 164 default + 162 no-ASR Rust; 135 frontend; 6 Python; clean Clippy/check; cache and16 tail fixtures | Complete |

## 3. Evidence and limits

Initial synthetic browser measurements found 385–456 ms history clear-search stalls
with 5,000 entries and 67,524 DOM nodes. The hidden idle overlay scheduled about 61
animation callbacks and 3,050 bar draws per second. These are Chromium measurements
with mocked Tauri IPC, not native WKWebView timings. Benchmark sources and updated
results are recorded in the [UI research](../research/2026-09-08-settings-performance.md).

Model measurements use public or synthetic fixtures, not private dictation history.
The expanded cleanup holdout disproved the earlier small-set quality conclusion;
unsafe candidates are not accepted merely for being newer. Vocabulary acoustic
scores also cannot reliably resolve genuine homophones. See the linked
[ASR](../research/2026-09-08-vocabulary-asr.md) and
[cleanup](../research/2026-09-08-small-cleanup-models.md) research for exact experiments.

Direct power counters require unavailable administrator authorization. CPU time,
latency, RSS, and Metal allocator measurements are not wattage. Native UI automation
was unavailable in this session; physical microphone, lock/wake, and paste behavior
must not be reported as passed solely from browser or injected-backend tests.


## 4. Pass 1 — integrated failure paths

Input fingerprint: `5d20037d470c3f68bf1c89d4be0ceabec807f3ab075b803afc5bda48767ea004`.
The initial integrated application compiled with 119 passing Rust tests and 109
passing frontend tests. Svelte/type checking reported no errors or warnings.
The primary reviewer combined code inspection with independent specialist findings
and inspected the actual 520×600 browser screenshots.

| Area | Review evidence and result |
| --- | --- |
| Audio/ASR | Checked shared worker boundary, capture/drain/WAV and retained original text; IPC start only emitted state without acquiring microphone; auxiliary download held the inference engine lock |
| Cleanup/vocabulary | 96-case cleanup test rejected the initial model/prompt; candidate development remains gated. Frozen acoustic policy improved4/48 independent clips without new errors; production Rust equivalence still to verify |
| Settings/UI/history | Inspected draft/save/setup generations, four sections and screenshots; normal Save unnecessarily blocked unrelated edits during setup; cleanup below first fold, low text contrast, stale history scroll |
| Persistence/clipboard | Initial atomic writes and failure-injection tests preserve disk/RAM; clipboard restore used a late change-count snapshot, corrected in this pass; Copy Last byte-sliced Unicode and logged text |
| Startup/exit/updater/packaging | Default-off/menu-bar setup and resource bundle inspected; tray refresh called blocking_lock from updater async tasks and could terminate the checker; quit-during-recording remains for subsequent lifecycle review |
| Privacy/accessibility | No cloud speech processing added; removed transcript logging at capture integration, found remaining Copy Last text logging; browser ARIA/keyboard tests distinguish native Accessibility validation |

Accepted corrections for this pass:

- **R1-1, high:** Prepare the auxiliary vocabulary resource outside ordinary ASR ownership and attach only after successful setup. A slow download must not prevent dictation.
- **R1-2, high:** Route the recording IPC command through actual microphone acquisition shared with the production/test paths, propagating errors and serializing duplicate starts.
- **R1-3, high:** Await updater-state locks and dispatch native tray mutations to the main thread. A regression exercises a contended lock on a single-thread async runtime.
- **R1-4, medium:** Copy Last clones text before background clipboard I/O and logs only a character count, removing the multibyte slicing panic and transcript leakage.
- **R1-5, medium:** Move cleanup to the first Dictation card, raise muted-text contrast, reset history scroll on page/search navigation, and permit saving unrelated preferences during setup while cleanup remains off.

Additional review 1 corrections propagated alternate-shortcut registration errors, validated all seven shortcuts using the native parser (including modifier aliases), atomically claimed update downloads and stop/cancel ownership, and captured clipboard restoration ownership immediately after our write. The UI also corrected inactive-tab references, late updater autoclose after teardown, loading/footer truthfulness, and the text-only clipboard restore description.

The initial pass 1 verification passed127 Rust tests, including detached setup not blocking ASR,48-case Rust policy equivalence, microphone failure/duplicate acquisition, stop/cancel contention, and async tray contention. The UI passed its focused8 tests and clean type/Svelte check; all four sections fit520×600 with125% enlarged text, keyboard navigation worked, and there were no browser page errors. Logs: `/tmp/sotto-review-1-rust.txt`, `/tmp/sotto-ui-review1-{check,tests,visual}.txt`. Final checks after correcting the clear/re-add setup race and duplicate-start flag preservation passed all 127 Rust tests in1.04s (`/tmp/sotto-review-1-final-rust.txt`). End fingerprint: `87d68cd7fc88ebf323bfa6a58f75fc2fe1e165bd07d6e0e83095d1e836a3fd71`, 06:42 UTC. The final Dictation screenshot was independently inspected by the primary reviewer. Pass1 is complete.

The SDK has a120s download-stall watchdog, a1,800s request timeout and four-attempt retry policy; these do **not** bound total setup/compilation time. Detached loading keeps ASR responsive, but the review does not claim a bounded total vocabulary preparation deadline. Native clipboard timing and physical lock/wake behavior remain unverified.


## 5. Pass 2 — independent interaction review

The frontend reviewer is independently covering the entire revised application,
including Rust/inference and packaging. The primary reviewer and subsystem owners
validate concrete findings and implement separately owned corrections.

Initial findings: an incomplete CTC cache can have model bundles and vocabulary but
lack a valid tokenizer; the SDK's early cached-return then makes Retry ineffective.
The now-hidden legacy max_history setting still rejected unrelated saves despite
having no runtime effect or editable control. Early LLM worker construction errors
can return before the child has an owner that kills and reaps it. Corrections and
final coverage evidence are in progress; this pass does not start pass3.


Pass 2 completed at 07:10 UTC. The independent frontend reviewer covered the entire
application and the primary reviewer checked integrated corrections. Initial Rust
compilation caught an obsolete settings loader after startup moved to checked loading;
the obsolete fallback API was removed and its tests now exercise the actual reader.
Final verification passed all **138 Rust tests** in 1.03 seconds after compilation.

| Area | Evidence, accepted finding and correction |
| --- | --- |
| Audio/ASR | Rechecked microphone ownership, callback drain, padded WAV, ASR offload, and original text. An old push-to-talk release could stop a new recording; release and auto-stop now compare the captured generation atomically. The stale 12.5-minute fallback now agrees with the 20-minute capture limit. |
| Cleanup/vocabulary | Cache status now validates integer tokenizer IDs/string merges. Missing or invalid tokenizer metadata is repaired through the actual SDK parser without deleting weights; seven offline Swift fault cases pass. Invalid/oversized sidecar protocol replies now terminate and discard the dead handle instead of restoring it for the next request. |
| Settings/UI/history | Rechecked draft intent, normalized Save acknowledgement, event disposal, pagination, failure notices and explicit destructive confirmation. The inert legacy history limit no longer blocks unrelated saves. CSV export now uses a private atomic native file write because the WebKit bundle did not have a browser-download handler. |
| Persistence/clipboard | Rechecked disk/RAM ordering and clipboard change-count ownership. Failed inference retains a private WAV and shows an error with Show audio/Dismiss. Failed paste reports clipboard availability only if fallback copying actually succeeds; history remains accessible. |
| Startup/exit/updater/packaging | Invalid shortcuts open Settings instead of aborting startup, partial registrations clean up, and updater preference contention cannot enable network checks. Actual Child ownership covers early construction failures and all temporary/resident workers; explicit Tauri exit kills/reaps registered process groups and rejects late spawns. |
| Privacy/accessibility | Native key capture is cancelled on Settings blur/destruction and validates focus before activation; key-name logging removed. Model links use native open_url. Error actions validate a regular app-named WAV in the system temporary directory; arbitrary paths/symlinks are rejected. Error cards fit 300×110 and late action completions cannot erase a newer error. |

Evidence: `/tmp/sotto-review-2-rust-final.txt`,
`/tmp/sotto-review2-vocabulary-cache-probe-fixed.log`,
`/tmp/sotto-ui-review2-tests.txt`, `/tmp/sotto-ui-review2-error-race-tests.txt`,
`/tmp/sotto-ui-review2-check-final.txt` and the review2 images under the UI experiment.
The primary reviewer inspected the transcription-failure card. The focused frontend
runs passed 49 and 4 tests, and Svelte/type checking reported no errors or warnings.
No private history or live Downloads folder was used by the persistence/export tests.
Retained audio is recoverable from the system temporary directory, not a permanent
archive; native microphone/lock/wake/paste remains an explicit final device-check limit.
End fingerprint: `bcd17fc75293677d05a05db1eee0bb8679a4ce016c62422c77753d4cb4aa7e52`.

## 6. Pass 3 — revised application review

Begins only after the integrated pass 2 checks above. The audio/ASR reviewer independently
covers the full application while the primary reviewer checks cross-component boundaries.


Pass 3 completed at 07:34 UTC with all **146 Rust tests** passing in 1.03 seconds
(`/tmp/sotto-review-3-rust.txt`). This includes the new device-error/Stop/Cancel
race, final sentinel samples at 192 kHz, stale callbacks, parsed physical PTT release,
retained audio until history acknowledgement, async clipboard responsiveness, and
unreadable-preference network guard regressions. Frontend checking remained clean;
ten focused overlay/waveform/history tests passed. The portable synthetic browser
benchmark passed with 50 history rows/707 nodes, zero hidden idle RAF/draws, and no
unknown IPC or page errors.

| Area | Evidence, correction and practical limit |
| --- | --- |
| Audio/ASR | A CPAL device error previously only logged and later pasted a prefix as a successful recording. A generation-bound one-shot callback now finishes that capture, drains the final samples, marks the transcript Interrupted, retains its private WAV, and skips cleanup/paste. The fixed 96kHz sample-count overflow path could discard an entire192kHz recording after10minutes; it is removed while the20-minute duration limit remains. Parsed shortcut codes now drive PTT release instead of unnormalized strings. |
| Cleanup/vocabulary | Rechecked raw source ownership, frozen48-case vocabulary policy, sidecar protocol deadlines, teardown and fallback. Shared subprocess ownership moved to process.rs and now also bounds the AppleScript activation fallback; one total deadline includes pipe drains. No failing model is promoted. |
| Settings/UI/history | Reduce Motion stops decorative transitions and waveform movement while preserving timer/status updates. Interrupted transcripts carry an optional backward-compatible capture_error and a visible badge. CSV now preserves quoted multiline text and interruption metadata. A history write error with the History window closed now shows recovery after output finishes and retains the WAV. |
| Persistence/clipboard | Original history/settings remain protected on read and write errors. Failed target activation previously still posted globalCmd+V; failures now propagate, the foreground PID is checked after the existing settle delay, and clipboard/history recovery handles failure. Blocking paste/copy runs on a worker instead of the async executor. Native delivery remains subject to OS focus changes and is not proven by injected-backend tests. |
| Startup/exit/updater/packaging | Corrupt settings can no longer apply temporary defaults to the OS login item or enable automatic update checks. Opening Settings/History previously changed the app to Regular activation policy; it now remains Accessory. Shared owned-child exit/deadline behavior is preserved. |
| Privacy/accessibility | Normal recording remains a non-key panel. Recovery errors alone allow keyboard focus; Dismiss/Escape and initial-button focus are guarded against Pasting and late events. Native keyboard behavior still requires a bundle/device check. No new remote speech processing, telemetry or private-history benchmarking. |

Apple documents that [Accessory apps can show and activate windows without a Dock
icon](https://developer.apple.com/documentation/appkit/nsapplication/activationpolicy-swift.enum/accessory),
that [activation can fail](https://developer.apple.com/documentation/appkit/nsrunningapplication/activate(options:)),
and that [NSRunningApplication properties are thread safe but update with the main
run loop](https://developer.apple.com/documentation/appkit/nsrunningapplication).
These ground the OS-policy correction and the limited post-activation check; they
do not establish an atomic focus-and-paste guarantee.

UI logs: `/tmp/sotto-ui-review3-check-final.txt`,
`/tmp/sotto-ui-review3-overlay-final.txt`, `/tmp/sotto-ui-review3-waveform-final.txt`,
`/tmp/sotto-ui-review3-tests.txt`, `/tmp/sotto-ui-portable-smoke-final.txt`.
End source fingerprint: `49808cea8316fb1d75c78749bfcf572a78ae1c0928e46744d192b8c3e74930e3`.

## 7. Pass 4 — independent runtime and interaction review

Begins after pass3's integrated checks. The cleanup/runtime reviewer covers all
application areas; model experiments remain separate from application-pass evidence.


| Area | Source-proven finding and implemented correction |
| --- | --- |
| Audio/ASR | Readiness still checked the legacy cache name instead of the SDK's actual v3 directory and required INT8 artifacts. Setup and engine now share the correct local readiness check; no cache was moved or deleted. Capture and shortcut transactions share one state lock, preventing microphone acquisition while a new shortcut set awaits persistence or rollback. |
| Cleanup/vocabulary | The sidecar request deadline began only after a potentially blocking stdin write. Nonblocking partial writes now share the absolute response deadline; a non-reading child is killed and reaped, and Unicode payload delivery is checked. Frozen vocabulary policy and independent evidence remain unchanged. |
| Settings/UI/history | An overlay could miss events during startup. A native revisioned snapshot retains phase, start time, generation and recovery error; the UI subscribes before fetching it and rejects stale updates. History add events now acknowledge only durably evicted IDs, preserving failed-save recovery entries and late initial-load arrivals. |
| Persistence/clipboard | Shortcut reconfiguration is excluded from recording until its save/rollback completes. At the existing history rollover limit, failed persistence retains every in-memory recovery entry; only a successful write truncates RAM and tells the UI which old IDs were removed. Existing arbitrary history files are not pruned during loading. Clipboard ownership and async delivery were rechecked. |
| Startup/exit/updater/packaging | Quit/restart could interrupt recording or transcription. Both now require Idle under the same lock as capture acquisition. A dedicated restart command checks before Tauri stores restart intent; the generic process plugin is removed from Rust, frontend and capabilities. Update/Onboarding show busy failures and allow retry. Tray diagnostics now reads a bounded log tail and runs system information/copying on a worker. |
| Privacy/accessibility | Overlay dismissal checks the exact revision inside the queued native action; delayed dismissal cannot clear a newer error. Native key status follows the retained error. Permission-reset errors and child deadlines are checked without exercising live TCC in tests. No personal transcripts or audio are used by benchmark fixtures. |

The primary reviewer inspected the actual 520×600 production Settings images:
cleanup is immediately reachable, setup is directly attached to its switch, and
Vocabulary explains words versus exact replacements. The production browser smoke
served the built bundle with the application's CSP and reported zero CSP violations,
page errors or unknown IPC calls. These remain synthetic browser checks, not native
WebKit timing or physical microphone tests.


Pass4 completed at08:07UTC after all **158 Rust tests** passed in1.00seconds
(`/tmp/sotto-review-4-complete.txt`). Initial compilation caught a missing Clone
bound on the new event payload and an obsolete unused store save method; both were
corrected before this final run. The permission reset tests inject timeout/nonzero
results and never reset live macOS permissions. The frontend passed26 focused tests
plus its guarded-restart regression and final zero-error/zero-warning check.
Production build/CSP smoke preceded the final restart import change; the release
build will cover that change again.

End fingerprint: `8eef57195ea621c6bf0ed3daf3b71ecd9d9dd1858f42c63a3bae817b7a25b1d1`.
The fingerprint utility does not include the capabilities directory, so its changed
file is recorded separately: `src-tauri/capabilities/default.json` SHA256
`f63233f7d413189818fc8696168e617eafdf8298b34e67d4009184219a863fa6`.

## 8. Pass 5 — final integrated application review

Starts after the complete pass4 checks above. The independent frontend reviewer
covers all application areas, with root reviewing cross-component delivery and
specialists checking the updated inference and capture boundaries. A model's failed
quality gate remains a product finding: experimental deletion suggestions must not
become automatic edits merely because they passed the structural byte-range guard.


| Area | Final review evidence, improvement and limits |
| --- | --- |
| Audio/ASR | Stream shutdown, final callback health, channel drain and WAV writing now run in one owned blocking worker across all four completion paths. A current-thread test proves a delayed Stop does not stall other async work, with a192kHz final sentinel intact. Direct CoreML loading replaces the SDK's destructive purge/retry path without changing the model, cache or compute policy. Three cache fault cases preserve artifacts; a real synthetic sentence through an isolated clone matches exactly. |
| Cleanup/vocabulary | All14 generative checkpoints and the specialist encoder failed automatic promotion. After three sequential amendment reviews, new AI results are separate experimental suggestions with Suggested status; ordinary output remains dictionary-adjusted ASR. An intentionally harmful but structurally valid deletion is injected in both paste and clipboard-only modes: neither changes delivered text. Independent source review also traced Copy Last, fallback Copy, events and legacy history. Vocabulary's frozen48-case policy is unchanged; final feature gating only excludes its unused implementation from builds without FluidAudio. |
| Settings/UI/history | Settings explains reviewed suggestions and preserves one-action setup/default-off/Save. Expanded History displays a separate escaped deletion diff and Copy suggestion, with independent successful-copy feedback; normal Copy and collapsed previews use the transcript. A delayed event for an already-deleted new row now still applies its durable old-row eviction acknowledgement. |
| Persistence/clipboard | Existing files and legacy Applied records remain readable without startup migration. Exports append the optional suggestion column and quote text/formula prefixes without changing stored text. CSV has no universal type metadata; spreadsheet resaving may remove formula protection. Native activation/change-count protections and captured-audio/history recovery were rechecked. |
| Startup/exit/updater/packaging | Automatic overlay hides can no longer clear a newer same-generation error; only matching Idle dismissal or a new recording clears recovery. Failed model-update responses now propagate errors instead of reporting up-to-date. Menu-only activation, restart-before-intent guard, owned-child exit, offline inference and disabled update checks were rechecked. |
| Privacy/accessibility | Neither model research nor regression tests uses private dictation. Production diff rendering escapes text; CSP smoke reports no violations or unknown IPC. Primary reviewer inspected Settings, suggestion review and recovery at actual window dimensions. Physical microphone unplug/sleep, focus-delivery races and energy consumption remain unmeasured. |

The integrated pass5 run passed **164 Rust tests** in1.05seconds
(`/tmp/sotto-review-5-rust.txt`). The frontend passed33 focused tests, clean checking,
and a production/CSP smoke that exercises explicit suggestion copying and failed
copy recovery. Root independently inspected the actual rendered images. Logs:
`/tmp/sotto-ui-review5-{tests,history-race,check-final,build,production}.txt` and
`/tmp/sotto-review5-asr-cache-probe.log`.

Consolidated verification then passed all **135 frontend tests** and **6 Python
protocol tests**. The first release script found four style lints, a test mutex held
across an await, and unused FluidAudio-only helpers in the no-ASR test build. These
were corrected without changing recognition or cleanup policy. The corrected
`cargo clippy --all-targets -- -D warnings` is clean and the no-ASR configuration
passes **162 Rust tests**, plus binary/doc-test targets. Passed checks were not
repeated solely to turn the earlier script's8/10 aggregate into a new summary.
Evidence: `/tmp/sotto-080-overnight-{pre-release,clippy-final,fallback-tests,frontend-tests,python-tests}.txt`.

CSV handling follows [OWASP's formula-injection guidance](https://owasp.org/www-community/attacks/CSV_Injection),
including its warning that protection is not universal across spreadsheet programs
and save/reopen cycles. The exported text marker is deliberate; the original
history remains unchanged. This source review and escaping test do not claim a
live Excel execution test.


Pass5 closes at08:26UTC. Final source fingerprint:
`d62fcb957c2d4c425975e8cff7ce313baf3043433317e53ee5864b412e2d21bf`;
capabilities remain at the separately recorded pass4 hash. All five application
reviews were sequential, with concrete implemented and verified changes between
every pass. The final16 synthetic52–62second quiet-tail/pause fixtures also pass
(`/tmp/sotto-080-overnight-asr-tail.txt`). Signed installation/startup is recorded
separately in the [installation journal](../journals/2026-09-08-local-080-install.md).


A final `cargo check --locked --no-default-features --features custom-protocol,asr-parakeet`
also passes after removing one preexisting unused response-length local in the
ONNX-only downloader (`/tmp/sotto-080-overnight-onnx-check-final.txt`). This code is
excluded from the installed FluidAudio build; no ASR model/configuration changed.
Final repository fingerprint after that optional-path cleanup:
`57cf3cc4450c16fb8cf577bd025ba244d7e9eb809f42e20a413449974fb865da`.
The installed Mac bundle corresponds to the preceding `d62fcb957c2d` fingerprint;
its selected source behavior is unchanged by this feature-excluded cleanup.
ONNX inference itself was not benchmarked or activated.
