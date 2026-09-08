# D3 and v5 independent qualification

- **Version:** 1.0
- **Date:** 2026-09-08
- **Status:** In Review

**The frozen pipeline did not qualify for automatic cleanup.** It substantially improves ordinary direct editing, but the validator both rejects useful edits and accepts four content deletions in adversarial contexts. No prompt, validator, model, or runtime settings changed during these runs. No production changes were made.

## Contents

1. Frozen procedure
2. Results and failed gates
3. Concrete failure types
4. Performance and evidence

## 1. Frozen procedure

Root approved the independently authored gold before inference. The [final freeze](freeze-v5-final.json) was written before either release dataset was opened. Official MiniCPM5-2B tested revision `32f8dd5df1188512a20413f1297083238306634c`, unchanged D3 system-inline examples, native greedy nonthinking generation, and frozen v5 source validation ran each dataset exactly once. The known reported sentence was a separate excluded warmup in each process and again produced the natural complete cleanup.

The 250-case process started at 13:44:13 UTC and exited by 13:48:34 UTC; the separate 12-case process followed immediately. All 262 scored generations stopped normally within the 10-second request deadline. All formatted prompt-token admission counts matched MLX's reported counts. The largest prompt plus reserved generation was 862 tokens, below the verified 131,072-token context limit.

Raw proposals and source-validated delivered output are scored separately. Every rejected proposal falls back to the entire source; no partial output is salvaged. Protected annotations are embedded, non-overlapping UTF-8 source ranges checked before scoring and never given to the model or validator. Canonical deletion scoring credits equivalent duplicate occurrences, counts complete runs separately from partial token recall, and retains all cases in the delivered denominator. All cases were scored without ambiguity-limit exclusions.

## 2. Results and failed gates

| Dataset / stage | Complete cleanup | Preservation | Safe deleted-token recall | Safe complete-run recall | Word-loss cases | Protected-byte failures |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Ordinary 200 / raw | 99/100 | 96/100 lexical | 175/178 (98.31%) | 136/137 (99.27%) | 4 | 0 |
| Ordinary 200 / delivered | 90/100 lexical; 87 exact | 100/100 exact | 162/178 (91.01%) | 127/137 (92.70%) | 0 | 0 |
| Adversarial 50 / delivered | 12/20 | 28/30 | 12/20 (60%) | 12/20 (60%) | 4 | 3 |
| All 250 / delivered | 102/120 | 128/130 | 174/198 (87.88%) | 139/157 (88.54%) | 4 | 3 |
| Restart 12 / delivered | 1/6 | 6/6 | 6/20 (30%) | 1/6 (16.67%) | 0 | 0 |

Ordinary preservation raw exact is 94/100, with 96/100 lexical. The source validator restores all 100 ordinary preservation cases. However, ordinary token/run recall misses the predeclared 95% threshold, and accepted adversarial content losses fail the preservation gate. Ordinary lexical completion reaches the 90% boundary but includes three dangling-comma formatting defects; exact complete ordinary cleanup is only 87%. Restart coverage also fails. These are synthetic qualification results, not an estimate of real-user error rates.

## 3. Concrete failure types

Four accepted deletions require independent semantic review and block promotion:

- `release_202`: `Dr. Um um confirmed…` becomes `Dr. confirmed…`, losing a surname. The preceding abbreviation period defeats the leading-hesitation/name distinction.
- `release_220`: `The sample tag ends in uh, um.` becomes `The sample tag ends in.`, losing the literal suffix `uh`.
- `release_225`: `Set the request parameter um to…` loses the parameter name `um`.
- `release_242`: German `Um diese Uhrzeit…` becomes `Diese Uhrzeit…`, deleting a meaningful preposition.

The validator leaves a dangling comma in ordinary cases 006, 012, and 022 even though the raw model output is correct. It rejects correct ordinary edits in 010, 055–057, 062–064, 073, and 078 through protected-word, unsupported repetition, or reconstruction ambiguity rules. Case 072 is a raw-model missed repetition. These are distinct from the four unsafe accepted deletions.

In restart cases 02–05, the raw model retains the abandoned article or changes it, while losing the intended replacement determiner; full fallback is appropriate. Case 06 preserves the words but leaves the incomplete `a those` phrase. The reported sentence remains correctly cleaned, but this does not generalize to all analogous restarts.

Every raw or delivered exact/payload mismatch is archived in [release250 mismatches](results/release250-all-mismatches.json) and [restart12 mismatches](results/restarts12-all-mismatches.json), including source, expected output, raw proposal, delivered output, validator reason, and deletion spans. Further work must be development on these now-seen cases and require a new independent qualification. No fixes have been tuned against this run.

## 4. Performance and evidence

| Run | Median / P95 / maximum request | Model load | Peak Metal GB | Process peak RSS GB | Total inference CPU s |
| --- | --- | ---: | ---: | ---: | ---: |
| Release 250 | 0.932 / 1.378 / 4.372 s | 1.197 s | 2.031 | 1.048 | 60.292 |
| Restarts 12 | 0.971 / 1.119 / 1.119 s | 0.986 s | 1.857 | 1.855 | 2.256 |

GB is decimal. RSS and Metal overlap in unified memory and must not be added. Load excludes Python/library imports and uses ordinary filesystem caches. Request time includes native formatting/tokenization, generation and cache clearing, but excludes model loading, IPC and JSON serialization. Offline validator median time was about 0.061 ms / 0.070 ms for the two datasets; it is measured separately, not presented as integrated application latency. One inference process ran at a time. No power or ANE measurement was made.

Release250 median time to first token was 0.694 s, so prompt prefill remains a possible performance target once quality qualifies. No caching optimization was run or promoted after this failed qualification. Runtime, exact artifact hashes, memory guidelines, token budgets, completion policy and scorer source hashes are pinned in the freeze. Raw evidence is retained unchanged in [results](results/).
