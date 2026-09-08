# D5: one inline abandoned-article example

- **Version:** 1.0
- **Date:** 2026-09-08
- **Status:** Implemented

This is development after the failed independent qualification. D3's semantic instructions, model, native template, decoding, and resource limits remain unchanged. The sole prompt change is a fourth inline example: `Please pack the uh um those blue spacers for tomorrow.` → `Please pack those blue spacers for tomorrow.`

The 36 pilot IDs and exact prompt were frozen before D5 inference. The pilot contains the reported sentence, eight original development cases, all twelve restart cases, the four accepted-loss cases from the first qualification, ordinary guard-rejection controls, multilingual preservation, and a long passage. D3's exact archived outputs provide the quality baseline; no mixed-date latency comparison is claimed. V5 remains unchanged, and raw/delivered outputs are scored separately without partial salvage.

| Metric | D3 | D5 |
| --- | ---: | ---: |
| Raw accepted exact /36 | 19 | 22 |
| Delivered accepted exact /36 | 18 | 20 |
| Complete raw restart cleanup /6 | 1 | 3 |
| Complete delivered restart cleanup /6 | 1 | 3 |
| Delivered restart preservation /6 | 6 | 6 |
| Accepted content-loss cases in pilot | 4 | 4 |

D5 fixes restart04 and restart06 and preserves the literal token in dev02. It makes one comma-placement change in dev08, whose whole proposal remains rejected for deleting literal content. All other raw outputs are identical to their archived D3 baseline. No regression was observed on this pilot, but three of six restart cleanups still fail, and the four unsafe accepted outputs remain unchanged. This is evidence of a partial causal improvement, not qualification.

All36 D5 requests completed within the deadline; median0.896s,maximum3.024s. The first reported sentence is deliberately included as a development sample and gets its accepted natural output. Its inconsistent primary gold is not suitable for canonical span recall; it remains separately identified. The old eight development cases and the sample have no independently authored protected-span annotations, and this absence is explicitly marked in the fixture rather than treated as proof of safety.

The next restart question is whether an explicit general rule retaining a replacement demonstrative resolves the remaining earlier-article preference. Literal/name/language protection is a distinct question and must not be conflated with the ordinary edits rejected by the validator. No production model or installed settings changed.

A separate D6 causal trial appends only: “When an article is clearly abandoned before empty hesitations and a replacement demonstrative, retain the later demonstrative and remove the earlier article together with those hesitations.” Its prompt and 36-case pass were frozen separately. D6 does **not** improve restart cleanup: it remains3/6. Raw accepted exact falls22→21; delivered exact rises20→21 from incidental German preservation, while raw quote deletion worsens and comma/capitalization formatting regresses. This does not support the abstract rule as a reliable repair of the targeted restart failure. All raw/evaluated D6 evidence is archived alongside the unchanged D5 results.

The same frozen D5 pilot was also run on cached Qwen3.5-4B, Spark4B, and the publisher-recommended Pollard mixed MiniCPM artifact. This is a development model-capacity control under the improved request shape; it does not replace the earlier independently frozen60-case comparison.

| D5 model | Raw exact /36 | Delivered exact /36 | Restart cleanup raw / delivered /6 | Restart preserve delivered /6 | Natural reported sentence | Median / maximum s |
| --- | ---: | ---: | --- | ---: | --- | --- |
| Stock Mini2 | 22 | 20 | 3 / 3 | 6 | Yes | 0.896 / 3.024 |
| Qwen4 | 22 | 22 | 4 / 3 | 6 | No | 1.673 / 5.707 |
| Spark4 | 25 | 20 | 3 / 2 | 5 | No | 1.643 / 5.833 |
| Pollard mixed | 21 | 20 | 3 / 3 | 6 | Yes | 0.931 / 3.863 |

Qwen4 retains the original four accepted-loss contexts. Spark fixes parameter and German cases but adds an accepted deletion of literal spoken words in restart11; surname and suffix losses remain. Pollard differs from stock on only two raw outputs (comma/capitalization). Qwen and Spark leave `all the yeah, those things` in the reported sentence: the old filler-only gold alternative accepts this, but it is not the natural cleanup target. All requests stop normally; no larger control establishes a qualification win. Spark's generic Transformers configuration warning is retained in its captured log; loaded MLX architecture metadata verifies the native `spark_mlx_llm.model.Model`, not a substituted Llama model.

D7 makes one further change to **D5**, not D6: a fifth mixed-role inline example, `Um, set the field named um to zero.` → `Set the field named um to zero.` It improves raw exact22→25 and delivered20→23. It fixes the mixed literal/repetition cases dev03/dev08, name dev07/release202, and parameter225; accepted-loss contexts fall from four to two (suffix220 and German242). However, restart04 regresses, reducing restart cleanup3/6→2/6. All36 D7 requests complete, with median0.916s and maximum3.470s. This is a useful causal semantic improvement with an explicit regression, not a passing configuration.

Exact pins, prompt/pilot freezes and raw/evaluated output accompany each profile. [Profile metrics](profile-comparison.json) also preserve separate RSS/Metal observations; those unified-memory figures overlap and must not be added. Timings are single serial runs on one Mac, not power measurements. New qualification2 data remains unopened pending final configuration freeze and root approval.
