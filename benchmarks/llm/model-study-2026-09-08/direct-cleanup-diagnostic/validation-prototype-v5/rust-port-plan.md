# Frozen validator v5: Rust port plan

- **Version:** 1.0
- **Date:** 2026-09-08
- **Status:** Draft

## Contents

1. [Contract and ownership](#1-contract-and-ownership)
2. [Implementation mapping](#2-implementation-mapping)
3. [Unicode compatibility](#3-unicode-compatibility)
4. [Parity and verification](#4-parity-and-verification)

## 1. Contract and ownership

This is a no-product-edit implementation plan, pending qualification review. Preserve the approved v5 source behavior; do not tune semantic rules during the port. The root agent owns runtime, cleanup, snapshots, pipelines, UI, and integration. The validator owner replaces `src-tauri/src/llm/edits.rs` after qualification, preserving any historical classifier adapter only in benchmark code.

The agreed production entry point is:

```rust
pub const MAX_CLEANUP_BYTES: usize = 32_000;
pub const MAX_CLEANUP_WORDS: usize = 1_024;
pub fn validate(source: &str, proposal: &str, protected_terms: &[String])
    -> Result<String, String>;
```

The caller accepts only a completed model response, invokes validation, and falls back to the entire source on error. Protected terms come from the same settings snapshot as deterministic replacement: canonical vocabulary and replacement values. The engine trait remains text-to-completed-text. No legacy candidate-ID gate should suppress sentence-initial, terminal, or phrase-restart cleanup.

An admission helper, if required, should share the byte and token limits. It must not approximate token count with `split_whitespace()`. Preserve validator no-change behavior, whole-proposal rejection, and the existing pure-hesitation empty-output case; any downstream empty-paste handling belongs to pipeline integration, not a silent validator-policy change.

## 2. Implementation mapping

| Frozen Python operation | Rust implementation |
|---|---|
| Word tokens and source locations | Borrowed `&str` tokens with UTF-8 byte ranges. Use the exact word class and internal apostrophe rule; compile fixed token patterns once. |
| Quote/code spans | Port the v5 scanner using `char_indices()`/byte boundaries. Track the complete backtick run length, escape parity, quote close character, and in-word apostrophe rule. Protect EOF if unmatched. |
| Identifier/dictionary lookarounds | Match fixed patterns/literals, then inspect adjacent Unicode characters with the same word predicate. Do not emulate lookarounds by consuming neighboring source characters. |
| Protected overlaps | Byte ranges plus a small internal reason enum; a token is locked on any overlap. Preserve exact source substring multiplicity for quotes/code, identifiers, and dictionary spans. |
| Repeated groups | A compact `{start, width, copies}` range descriptor for the same 1–4-word adjacent patterns; full-fold comparisons and the same distinct-word/function-word conditions. |
| Article restarts | The same bounded source index range plus retained replacement index. No added restart heuristics. |
| Alignment | Explicit DFS work stack with source/proposal indices and retained source indices. Push deletion before match so traversal and first reconstruction remain deterministic. Bound work at 20,000 popped states and 128 complete alignments, preserving exact threshold semantics. |
| Allowed deletion union | Per-token booleans; require the union of allowed complete hesitation/repeat/restart deletions to equal the proposal's deleted set. No partial proposal salvage. |
| Reconstruction | Merge source byte intervals; copy untouched source slices. Only the established local paired-em-dash rule inserts one ASCII space. Record source byte offsets for narrow ASCII sentence-initial capitalization. |
| Equivalent alignments | Accept only when every accepted alignment yields the same delivered string. Materially different punctuation reconstruction rejects the whole proposal. |
| Comparison punctuation | Source-owned comma/spacing policy, optional final period, canonical paired quote delimiter style, and the identified em-dash join. Preserve exact quote payload and every newline; no generic punctuation normalization. |

The existing dependencies already provide `serde`, `serde_json`, and normal standard-library containers. `regex 1.12.3` is locked and cached transitively, but must be declared directly before use. Its cached `regex-syntax 0.8.10` tables explicitly identify Unicode 16.0.0. Do not add a backtracking-regex crate: the supported Rust regex engine intentionally omits lookarounds/backreferences, and the boundary/scanner logic is small. [Regex crate documentation](https://docs.rs/regex/1.12.3/regex/).

## 3. Unicode compatibility

Python `\w` is `str.isalnum()` plus underscore, and `\d` is decimal category `Nd`. Its whitespace matching follows `str.isspace()`. Rust regex `\w` is broader and includes combining marks, other connector punctuation, and join controls. Use explicit Letter/Number/underscore classification and manual matching boundaries; do not substitute Rust regex `\w`, `\b`, or `char::is_alphanumeric()` without parity evidence. [Python regex definitions](https://docs.python.org/3/library/re.html), [Rust Unicode classes](https://docs.rs/regex/1.12.3/regex/#perl-character-classes-unicode-friendly), [Rust character classification](https://doc.rust-lang.org/std/primitive.char.html#method.is_alphanumeric).

Full case folding is distinct from lowercase conversion: for example, `ß` folds to `ss`. Retained word equality remains exact except for the narrow ASCII capitalization rule, but repeat-group recognition and static-token membership still use folded keys. Preserve those equivalences without allowing model substitutions. [Python case folding](https://docs.python.org/3/library/stdtypes.html#str.casefold).

The preferred compact dependency is **`focaccia = { version = "=1.5.0", default-features = false }`**, using `unicode_full_case_eq` for token-key comparisons. The actual published archive was inspected: Unicode 16.0.0, no dependencies, Rust 1.76 minimum, MIT plus Unicode-3.0 license, 54,685-byte source archive. This fits the project's declared Rust 1.80 minimum and avoids a custom case table. [Published crate archive](https://static.crates.io/crates/focaccia/focaccia-1.5.0.crate), [versioned source manifest](https://github.com/artichoke/focaccia/blob/v1.5.0/Cargo.toml).

Do not select `unicode-casefold 0.2.0` (its packaged table is Unicode 9), or unpinned latest Focaccia (2.1.0 uses Unicode 17). `caseless 0.2.2` supplies Unicode 16 folding but adds normalization dependencies that this validator does not need. The intended operation is ordinary full default folding, never Turkic folding or Unicode normalization. [Focaccia current Unicode policy](https://docs.rs/focaccia/2.1.0/focaccia/).

Two details require explicit parity fixtures before selecting helpers:

- Dictionary literal matching uses Python `re.IGNORECASE`, not full multi-character case folding. Preserve its one-to-one matching, including dotted/dotless I behavior, and inspect word boundaries manually. Do not make a `Strasse` term match `Straße` merely by substituting full-fold substring search.
- Python `isdigit()`, `isspace()`, first-character `isupper()`, and string `lower()` are separate operations. Check superscript digits, ASCII control separators, titlecase characters, final sigma, combining marks, and new Unicode-16 characters. Avoid conflating them with Rust's broader numeric or alphabetic predicates.

Authored v5 checks ran on Python 3.11.14 / Unicode 14. Qualification uses Python 3.14 / Unicode 16, which is the intended production parity reference. Record both; do not claim the interpreter change has no effect on all possible text. Generate the port's Unicode oracle with the qualification interpreter and compare the finite development fixtures across both interpreters. [Python 3.14 Unicode version](https://docs.python.org/3.14/library/unicodedata.html).

## 4. Parity and verification

The gate is behavior equivalence to the frozen Python source, not just an aggregate benchmark score:

1. Replay all **567 existing development proposals**. Compare acceptance, exact delivered bytes, category membership, and independently reconstructed source intervals/one-space insertions/capitalization. Preserve deterministic traversal so equivalent deletion intervals can also be compared.
2. Export the **24 authored test groups** into a portable case fixture, including every loop-generated delimiter, escape, dictionary, Unicode-offset, boundary, and limit case. Keep expected outputs from the frozen validator and assertions; do not include release250/restart12 content in this export.
3. Add a small language-parity oracle for characters and strings: `ß/ss`, Greek sigma variants, ligatures, dotted/dotless I, long s, Kelvin sign, composed/decomposed accents, combining U+0345, join U+200D, connector U+203F, Arabic decimal digits, superscript digits, nonbreaking space, and ASCII record separators. Include token byte boundaries, folded equality, and protected dictionary spans.
4. Check byte/word bounds exactly at and above the limits, alignment-state/count rejection, unchanged text, empty pure-hesitation output, ambiguous repetitions, and malformed/uncompleted response handling in the caller. Rust `&str` is valid UTF-8, so malformed Unicode belongs at deserialization/runtime boundaries.
5. Run targeted Rust parity/unit tests once after implementation with `tee`; then the root runs the integrated build, clippy, and full tests. No MLX, app launch, or model downloads are needed for validator parity tests.

No Rust product files or dependencies were changed while preparing this plan. The Unicode fixtures and any direct dependency additions remain implementation prerequisites after qualification approval; unresolved mismatches must be reported, never silently accepted as policy changes.
