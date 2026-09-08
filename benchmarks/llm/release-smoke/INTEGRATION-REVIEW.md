# MiniCPM automatic cleanup integration review

- **Version:** 1.0
- **Date:** 2026-09-08
- **Status:** In Review

This independent source review covers the user-requested local MiniCPM test release. It does not supersede the failed D3 model qualification or characterize the structural validator as a semantic guarantee.

The production hotkey path and testable pipeline now change delivered `final_text` only for a validated `Applied` result. That same text feeds History, word count, clipboard, paste, and Copy Last. The original ASR text remains in `raw_text` when the delivered text differs. Legacy `Suggested` history data remains readable; new automatic entries do not create a parallel suggestion.

Both paths pass the vocabulary and dictionary replacement strings to source validation. Failed, incomplete, oversized, unsupported, and ambiguous proposals fall back to the full pre-cleanup transcript. Reliable non-English detection abstains before inference; short ambiguous foreign input remains an acknowledged limitation. A completed empty proposal can validly remove an all-filler recording: the history entry retains its raw text, and the pipeline does not paste or overwrite the clipboard with empty text.

The post-cleanup stale-job check now occurs before publishing the cached cleanup status or saving/delivering text. The regression invalidates the job from a fake cleanup backend before returning a valid changed proposal, then verifies that no cleanup status, transcription, paste, or clipboard result escapes and that the newer recording state survives. This is a targeted regression, not a claim of exhaustive application concurrency verification.

The bundled sidecar matches the D7 inline-example prompt and official MiniCPM checkpoint. It preserves native chat-template/BOS handling, greedy non-thinking generation, complete-stop enforcement, context admission, a ten-second generation deadline, and bounded input/output lines. Load/preparation also validates returned model ID, pinned revision, and prompt hash. A display-name mismatch identified in the initial review was corrected before verification.

The 0.8.3 follow-up adds an explicit prewarm readiness contract: successful load returns `warmed:true`; `did_warm:true` identifies the one call that performed fixed synthetic inference, and repeated loads return `did_warm:false`. Status reports the warmed state, and Rust readiness must reject a merely allocated model. The smoke now checks cold load/warm, resident load reuse, and first user request timing separately. Implementation and bundled measurements remain pending the release owner's build handoff; this paragraph records the reviewed API, not a completed warmup result.

The Rust validator author reports five focused tests passed, including all 672 exported frozen v6 oracle cases and additional Unicode/dictionary boundary probes. The parity export was independently generated from authored tests and 21 revealed profiles, without reading new qualification data. Bundled execution and full native integration checks are recorded separately by the release owner; no inference was run during this source review.
