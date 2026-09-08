# Cleanup detection and Settings close follow-up

- **Version:** 1.0
- **Date:** 2026-09-08
- **Status:** Implemented

## Contents

1. Summary
2. Problem statement
3. Design overview
4. Detailed design
5. Edge cases
6. File changes
7. Testing strategy
8. Migration plan
9. Security considerations
10. Cost analysis
11. Implementation tasks
12. Implementation status

## 1. Summary

Fix two reported regressions in the locally installed 0.8.0: unpunctuated
hesitations never reached cleanup inference, and the Settings window's native
close button required a missing Tauri permission.

## 2. Problem statement

The user's supplied sentence contains three standalone `um` tokens and one `uh`.
Its saved status is `no_changes`, with no suggestion. Logs confirm that the
download and model load succeeded at 11:40:57–58 UTC. The candidate detector
requires a comma, so it returns before inference for this input.

The installed Tauri JavaScript SDK's `onCloseRequested()` calls `destroy()`
after an allowed close. Our Settings listener uses that API, but the capability
grants only `close()`. Existing mocks did not exercise the SDK's follow-on call.

## 3. Design overview

Accept standalone lowercase hesitation tokens with or without a comma. These
are proposals, not evidence that deletion is semantically safe. Preserve the
current review-only delivery contract unless the user chooses automatic use.
Distinguish candidate-free skips from a model choosing no deletions. Grant
destruction only to the Settings window that needs it.

## 4. Detailed design

Keep the existing exact token vocabulary (`um`, `uh`, `uhm`, `erm`), quote/code
exclusions, uppercase-name protection, byte-preserving reconstruction, size
limits and final-token protection. Do not add substring or fuzzy removal.
Leave `yeah` alone: it can convey agreement or emphasis.

Add `skipped_no_candidates` as a persisted cleanup outcome. The History UI
explains whether the model was skipped or ran without choosing removals.
For legacy `no_changes` records use neutral wording such as “Cleanup made no
changes,” because older versions used that status for both detector skips and
model no-ops. Only the new `skipped_no_candidates` status states that inference
did not run. Return candidate-free results before acquiring the shared model
operation lock or starting/loading the runtime; they must not wait behind model
setup. Propagate the new status through persisted history, runtime status, overlay
and frontend exhaustive matches. Log only candidate counts and outcomes, never
transcript text.
The empty result also covers transcript/candidate limits, so its UI explanation
must include that possibility rather than claiming that no fillers were present.

Add a Settings-only capability permitting `core:window:allow-destroy`. Test
the real SDK close listener with the configured permissions, plus dirty-draft
Keep editing, Save, Discard and failed-save behavior.
Clear the preparation notice asking the user to Save once cleanup has actually
been saved as enabled; failed or unrelated saves must not claim activation.

## 5. Edge cases

Unquoted words being discussed and German/Portuguese `um` remain semantically
ambiguous; an expanded candidate set cannot guarantee safe model decisions.
Quoted mentions, identifiers, uppercase names, paragraph breaks, Unicode,
terminal fillers and oversized inputs retain existing protections. Runtime
failure preserves ordinary output and records its reason.

## 6. File changes

- `src-tauri/src/llm/{edits,cleanup}.rs`, `models.rs`, `hotkeys/manager.rs`, `pipeline.rs`.
- `src/lib/utils/tauri.ts` and `components/history-item.svelte` with its tests.
- `src-tauri/capabilities/settings-close.json` and `src/lib/utils/window-close.test.ts`.
- `src/lib/components/settings-panel.svelte` and its tests for close and setup notice.
- `src-tauri/sidecar/llm_cleanup.py` only if a frozen prompt improves measured suggestions.
- Focused regression tests and this follow-up record.

## 7. Testing strategy

Exercise the user's exact sentence through candidate enumeration, reconstruction,
mocked runtime invocation and the actual installed MLX runtime. A mock selecting
all four hesitation candidates must produce a Suggested result while preserving
the remainder and final words exactly. A candidate-free test must prove that the
backend is never invoked, even while the model operation lock is held. Include
new/legacy cleanup-status serialization and a deliberately harmful multilingual
mock to verify that ordinary transcript, Copy and paste still preserve the input. Use independent
literal/quoted/identifier/repeat cases to check retained boundaries. This is a
regression probe, not a renewed claim that the model passed the overnight gate.

The stock prompt, given all four candidates, selects only the last `uh` on the
reported sentence. Compare at most three prompt-only variants on existing 96
regression cases, then freeze one before opening 16 independently authored cases.
Keep model identity and runtime fixed. Preserve the independently authored
holdout file and SHA before prompt selection; no prompt or detector tuning after
reading its labels. Report exact matches, useful edits and harmful deletions
separately, with the unchanged review-only output boundary checked even when a
model removes a meaningful literal or foreign-language word. A narrow prompt improvement may improve
suggestions; it does not erase the earlier automatic-delivery failures.

Run Rust tests and strict Clippy, frontend tests and type checks, then a signed
production build. Install with an application backup and preserve settings and
history. Verify the version, signature and startup log.

## 8. Migration plan

No settings or history rewrite. Existing cleanup statuses remain readable.
The new status is only written for subsequent recordings. Model identity,
weights, runtime and cache locations stay unchanged.

## 9. Security considerations

Restrict the new capability to Settings. No network activity is introduced at
inference, no external transcript processing, and no changes to credentials.

## 10. Cost analysis

Previously skipped bare fillers now incur the existing bounded local inference
cost. Candidate-free text still returns immediately. No added model download.

## 11. Implementation tasks

- [x] Review assumptions, completeness, then actionability in three sequential passes.
- [x] Fix punctuation-independent candidates and distinguish skipped inference.
- [x] Fix Settings native close and cover actual SDK lifecycle.
- [x] Probe the exact reported sentence with the installed local model.
- [x] Compare bounded prompt variants; validate a frozen choice on withheld cases.
- [x] Preserve existing review-only delivery while the optional preference question is unanswered.
- [x] Run checks, build, back up and install, then record verification and native retest limit.

## 12. Implementation status

Implemented and installed as local 0.8.1. All 168 Rust, 145 frontend and six Python
tests pass, with clean lint/type checks and signed production build. See the
[installation and verification record](../journals/2026-09-08-local-081-install.md).
The Settings X failure was reproduced natively before replacement; the post-install
native retest was blocked by computer-use access to the menu-bar-only app. The
actual SDK close path and unsaved-draft flows pass automated regressions.

Model quality remains limited: the selected prompt improves useful suggestions
on known regression cases, but leaves three fillers in the reported sentence and
still makes harmful suggestions on independent validation. No automatic-delivery
promotion occurred. A subsequent live recording on the installed app successfully
ran inference and saved a suggestion in 184 ms.

Review 1 — Assumption validation: the independent cleanup reviewer verified both
code paths and required the prompt experiment above because candidate coverage
alone only removes one of the four hesitations with the current model.


Review 2 — Completeness: the recording/backend reviewer completed this pass after
review 1 and after freezing 16 new held-out labels without viewing the prompt
variants. Added truthful legacy-status wording, skip-before-lock/runtime behavior,
all status consumers, four-candidate/tail reconstruction and harmful-output
containment regressions, and a no-tuning-after-holdout boundary. Confirmed that
permission scope remains Settings-only and existing dirty-draft, failed-save,
model identity, preferences and history migration semantics are preserved.
Actionability review remains pending; no product source changed during this pass.

Review 3 — Clarity and actionability: root checked the revised test and migration
contracts after review 2, added exact file ownership and the stale setup-notice
fix observed in native Settings, and confirmed that prompt promotion is conditional
on results. Native UI access is now available: reproduce X before installation
and verify it closes the installed replacement. Approved for implementation.
