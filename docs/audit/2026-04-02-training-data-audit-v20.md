# Training Data Audit — v20 (101,954 entries)

- **Version:** 1.0
- **Date:** 2026-04-02
- **Status:** Draft
- **Dataset:** `~/sotto-finetune/data_v20_final/train.jsonl` on 192.168.1.128

## Table of Contents

1. [Executive Summary](#1-executive-summary)
2. [Critical: Grammar Errors in Clean Output](#2-critical-grammar-errors-in-clean-output)
3. [Critical: "We've" Misused as Main Verb](#3-critical-weve-misused-as-main-verb)
4. [High: Missing Commas (Subordinate Clauses and Compound Sentences)](#4-high-missing-commas)
5. [High: Repeated Words Surviving Cleanup](#5-high-repeated-words-surviving-cleanup)
6. [High: Dictation Artifacts in Output](#6-high-dictation-artifacts-in-output)
7. [Medium: Number and Percent Formatting Inconsistency](#7-medium-number-and-percent-formatting-inconsistency)
8. [Medium: Over-Cleaning — Hedges and Meaning Removed](#8-medium-over-cleaning)
9. [Medium: Tone Formalization](#9-medium-tone-formalization)
10. [Medium: Lowercase Proper Nouns and Acronyms](#10-medium-lowercase-proper-nouns-and-acronyms)
11. [Low: Miscellaneous Issues](#11-low-miscellaneous-issues)
12. [Clean: Areas That Passed](#12-clean-areas-that-passed)
13. [Summary Table](#13-summary-table)
14. [Recommendations](#14-recommendations)

---

## 1. Executive Summary

The dataset is in significantly better shape than previous versions — whitespace/encoding is spotless, smart quotes are gone, and most filler removal works well. However, this audit found **several systematic errors that are actively teaching the model wrong grammar and wrong formatting conventions.** The two most damaging are:

1. **398 entries with "X and I am"** — a conjugation error the model will learn and reproduce.
2. **~2,000 entries with "We've" used as the main verb "We have"** — grammatically awkward British English that American English speakers will perceive as broken.

Beyond these, there are ~2,227 entries missing commas before "but" in compound sentences, ~411 missing commas after subordinate clauses, ~300 "the the" stutters that survived into the output, and ~290 dictation word artifacts ("period", "question mark" as literal text). Number formatting is deeply inconsistent: `%` vs `percent`, `AM` vs `am`, large numbers with and without commas.

Total estimated bad entries: **~7,000–8,000** (some entries have multiple issues).

---

## 2. Critical: Grammar Errors in Clean Output

### 2.1 "X and I am" — 398 entries

When the input has "me and X" (e.g., "me and sarah gonna present"), the cleanup correctly reorders to "Sarah and I" but then conjugates with "am" instead of "are". This is a **systematic bug**, not a random error — every instance of this pattern is wrong.

| Entry | Input | Output |
|-------|-------|--------|
| 642 | `me and her is gonna fix this for the sprint` | `She and I am going to fix this for the sprint.` |
| 903 | `he and me are responsible for the client onboarding` | `He and I am responsible for the client onboarding process.` |
| 945 | `me and sarah gonna present the nps data tomorrow` | `Sarah and I am going to present the NPS data tomorrow.` |
| 1601 | `im pretty sure their were three people who said they cant make it but me and sarah are going` | `I'm pretty sure there were three people who said they can't make it, but Sarah and I am going.` |
| 3730 | `me and the devops team are gonna implement the new monitoring tools today` | `The DevOps team and I am going to implement the new monitoring tools today.` |

**Every one of these should use "are" not "am".** "Sarah and I" is a plural subject.

Related: One entry also shortens "Jessica" to "Jess" (entry 66842: `Jess and I am going to fix...`).

### 2.2 Subject-Verb Disagreement — ~8 entries

A smaller set of entries has other subject-verb errors:

| Entry | Output | Should Be |
|-------|--------|-----------|
| 8975 | `the team need to fix it` | `the team needs to fix it` |
| 17822 | `The client presentation need to show...` | `needs to show` |
| 68570 | `The team need to update the DCF model` | `needs to update` |
| 96899 | `Diabetes patient need to follow up` | `needs to follow up` |

### 2.3 Missing Articles / Broken Grammar

| Entry | Output | Problem |
|-------|--------|---------|
| 64176 | `I just want to borrow your phone for moment.` | Missing "a" — "for a moment" |
| 67181 | `You're a very generous person and I like work with you.` | Should be "I like working with you" |
| 71806 | `My child is sick and I take him to doctor.` | Should be "I'm taking him to the doctor" |
| 9897 | `The database schema needs a update.` | Should be "an update" |
| 82174 | `We should aim for a 18 gross margin` | Should be "an 18% gross margin" |

---

## 3. Critical: "We've" Misused as Main Verb

**~2,000 entries** contract "We have" to "We've" when "have" is the main verb (possession/existence), not an auxiliary. While technically not wrong in British English, it sounds unnatural in American English and is especially jarring in short sentences:

| Entry | Output | Natural Form |
|-------|--------|-------------|
| 191 | `We've a lot of work on amortization before the deadline.` | `We have a lot of work...` |
| 579 | `We've six candidates.` | `We have six candidates.` |
| 746 | `We've three pending approvals.` | `We have three pending approvals.` |
| 918 | `We've four priorities here: 1. Stabilize the API layer...` | `We have four priorities...` |
| 744 | `We've a patient with diabetes and hypertension` | `We have a patient...` |

For comparison, the dataset has ~1,602 entries where "We've" is correctly used as an auxiliary ("We've been tracking...", "We've already deployed..."). The split is roughly 55% wrong / 45% right, meaning the model will learn to produce this awkward form regularly.

Also applies to "we've" in other positions:
- `We've high NPS but MRR is flat.` (entry 356)
- `We've too many pods running.` (entry 499 equivalent)

---

## 4. High: Missing Commas

### 4.1 Missing Comma After Subordinate Clauses — ~411 entries

When a sentence begins with a subordinate clause ("If...", "When...", "After...", "Before...", "Since..."), standard punctuation requires a comma before the main clause. These are missing:

| Entry | Output |
|-------|--------|
| 54 | `If we get a subpoena we have to disclose the data.` |
| 265 | `If we restart the nodes now the system might be unstable so we should wait an hour.` |
| 1123 | `If the defendant fails to comply we have to file a motion.` |
| 5368 | `If we proceed with this clause it could delay the trial.` |
| 8279 | `If we don't act now we lose our market share.` |

### 4.2 Missing Comma Before "But" — ~2,227 entries

In compound sentences (independent clause + "but" + independent clause), a comma before "but" is standard. The dataset has **zero** entries with a correctly placed comma before "but" (the regex found 0 `", but"` patterns using the method tested). All 2,227 compound-sentence uses of "but" lack the comma:

| Entry | Output |
|-------|--------|
| 105 | `We've been waiting for the client feedback but they said maybe next week.` |
| 162 | `The server is responding but the latency is high.` |
| 193 | `The free cash flow projections look stable but the debt service coverage ratio is under pressure.` |

### 4.3 Tag Questions Without Comma — ~115 entries

Tag questions like "right?" at the end of a sentence need a comma separator:

| Entry | Output | Should Be |
|-------|--------|-----------|
| 65 | `We should probably check the logs right?` | `...the logs, right?` |
| 1213 | `We should probably add input validation right?` | `...validation, right?` |
| 3964 | `If we receive a subpoena the compliance officer should be notified immediately right?` | `...immediately, right?` |

---

## 5. High: Repeated Words Surviving Cleanup

### 5.1 "The the" — ~300 entries

Input stutters where "the the" appears are being passed through to the output instead of being cleaned:

| Entry | Input | Output |
|-------|-------|--------|
| 529 | `the the max retries is two no wait more like four around four` | `The the max retries is around four.` |
| 756 | `the the timeout is 100 sorry more like 500 around 500` | `The the timeout is around 500.` |
| 1372 | `the the budget is 50 actually more like 200 around 200` | `The the budget is around 200.` |

This appears to be a systematic pattern from a specific template — all follow the form "the the X is Y actually more like Z around Z" and the cleanup fails identically every time.

### 5.2 Other Repeated Words — ~60 entries

| Entry | Output | Problem |
|-------|--------|---------|
| 146 | `Can you clarify the I R R assumptions?` | Should be "IRR" |
| 2408 | `The API endpoint is slash slash slash slash API dot example dot com slash slash v1...` | URL spelled out as words with chaotic repetition |
| Sample 24 | `So we actually hit hit our NPS target of 72...` | "hit hit" stutter survived |

---

## 6. High: Dictation Artifacts in Output

The inputs contain spoken punctuation commands ("period", "comma", "question mark") that the model should convert to actual punctuation. In ~290 cases, these words survive into the output as literal text:

| Entry | Output | Problem |
|-------|--------|---------|
| 4898 | `...got that question mark?` | "question mark" is literal text, not just `?` |
| 34527 | `We lost a client yesterday period, that spikes the churn rate to 5% period.` | Two literal "period" words |
| 67378 | `I think we're over budget period, let me check the dashboard question mark.` | Both "period" and "question mark" literal |
| 71714 | `Is the new hiring manager online yet question mark, if not send them a reminder period?` | Full confusion |
| 5665 | `Check the Q2 MRR growth period, please.` | Ambiguous: could be fiscal period or artifact |

The ambiguity problem is real — "trial period", "follow-up period", "Q4 period" are legitimate uses of the word. But entries like `wait for the build and check the period` (entry 1545) are clearly artifacts.

### 6.1 Literal "slash" in Output — ~257 entries

Many of these are legitimate (e.g., `confidence interval/effect size`), but some are clearly artifacts:

| Entry | Output | Problem |
|-------|--------|---------|
| 2408 | `The API endpoint is slash slash slash slash API dot example dot com...` | URL should be `//api.example.com/v1/users` |
| 874 | `The interview schedule slash one two three, four five six.` | Garbled |
| 1904 | `The VLAN is one zero five, but the subnet is twenty four dot slash thirty two.` | Should be `24/32` |

---

## 7. Medium: Number and Percent Formatting Inconsistency

### 7.1 Percent Symbol vs Word — Split Decision

The dataset uses `%` in **1,748** entries and spells out "percent" in **1,532** entries. There is no consistent rule — sometimes even the same type of content uses different formats.

### 7.2 Large Numbers — Comma vs None

- With comma (10,000): **613** entries
- Without comma (10000): **691** entries

Many "no comma" entries are ambiguous (ports like 8080, 9000) but genuine large numbers like `45678`, `12345`, `5000` (as dollar amounts) appear without commas.

### 7.3 Time Format — AM/PM Case

- `AM`/`PM` (uppercase): **220** entries
- `am`/`pm` (lowercase): **260** entries (including mixed like `4pm`, `9 am`)
- `a.m.`/`p.m.`: **0** entries

No standard chosen.

### 7.4 Mixed Spelled/Digit Numbers — ~2,016 entries

Many entries mix spelled-out and digit numbers in the same output:

| Entry | Output |
|-------|--------|
| 2 | `The results are: 1. Latency went up to five hundred ms 2. Error rate hit 10% 3. The CPU usage spiked.` |

Most of these are the numbered-list pattern (1. 2. 3. combined with spelled-out quantities), which may be acceptable, but some are truly inconsistent.

### 7.5 Ordinals — Mixed

- Numeric ordinals (1st, 2nd, 3rd): **181** entries
- Spelled ordinals (first, second, third): **2,650** entries

No clear rule for when to use which.

---

## 8. Medium: Over-Cleaning

### 8.1 Hedges Removed — ~928 entries

Hedging words ("I think", "probably", "maybe", "might") were removed in ~928 entries. Many removals are fine (speaker restarts), but some change meaning:

| Entry | Input | Output | Problem |
|-------|-------|--------|---------|
| 334 | `we dont have enough funds um to hire developers or maybe we do` | `We don't have enough funds to hire developers.` | Removed "or maybe we do" — flipped the uncertainty |
| 1454 | `maybe we could stay home and watch a series but that sounds boring so let's find a board game night instead` | `Let's find a board game night instead.` | Lost the deliberation entirely |
| 2236 | `delegate this to the finance team or maybe we should just handle the cost reduction ourselves` | `Delegate this to the finance team.` | Removed the preferred option |
| 4337 | `schedule the echocardiogram tomorrow or maybe today depending on availability` | `Schedule the echocardiogram today depending on availability.` | Removed tomorrow as an option but kept "depending on availability" (contradictory) |
| 3684 | `the interaction term... was marginally significant at p equals 0.067 so so we we probably should include it...` | `...so we should include it...` | "probably" matters in scientific context |

### 8.2 Self-Correction Gone Wrong — Semantic Errors

Some self-corrections are applied incorrectly:

| Entry | Input | Output | Problem |
|-------|-------|--------|---------|
| 72 | `if we subpoena the witness uh no wait the court denied the subpoena because of privilege issues` | `If we subpoena the witness, the court denied it because of privilege issues.` | Garbled — merged two thoughts nonsensically |
| 379 | `im going to um run the test suite no wait let me check the logs first` | `I'm going to run the test suite.` | Ignored the correction! Speaker wanted to check logs first |
| 436 | `the plaintiff needs the documents wait actually I mean the defendant needs to produce the discovery` | `The plaintiff needs the documents.` | Ignored the correction entirely |
| 301 | `we need to finalize the Q4 pipeline strategy wait actually the Q3 pipeline is the one needing attention...` | `We need to finalize the Q4 pipeline strategy.` | Kept the wrong version |
| 130 (full) | `the variance was due to currency fluctuation wait actually no we need to adjust for inflation rates first so the adjusted delta is higher` | `The variance was due to inflation rates first so the adjusted delta is higher.` | Garbled merge |

---

## 9. Medium: Tone Formalization

**~3,568 entries** systematically formalize casual speech:

| Casual | Formal | Count |
|--------|--------|-------|
| gonna | going to | ~2,500+ |
| wanna | want to | ~400+ |
| gotta | have to / got to | ~300+ |
| kinda | kind of | ~100+ |
| cause | because | ~100+ |

This is a **design decision**, not necessarily an error. However, it means the model will never output "gonna" even when the speaker's register clearly calls for it. A dictation tool should arguably preserve register for casual messages.

---

## 10. Medium: Lowercase Proper Nouns and Acronyms

### 10.1 "go" (the Language) — ~33 entries

When "go" refers to the Go programming language, it should be capitalized:

| Entry | Output |
|-------|--------|
| 1136 | `Wait the go service just crashed.` |
| 4143 | `It's due to inefficient serialization in the go code.` |
| 4225 | `Can we merge the go module into the main repo?` |

### 10.2 "mrr" / "us" — ~75 entries

| Entry | Output | Should Be |
|-------|--------|-----------|
| 5065 | `The mrr projection is too optimistic...` | `MRR` |
| 3086 | `The server is in the us for this case.` | `US` |
| 6579 | `The latency increased on the us west region.` | `US` |

### 10.3 Capital "I" in Code — 5 entries

| Entry | Output |
|-------|--------|
| 8380 | `for (let i = 0; I < n; I++)` |

The autocapitalization of `i` to `I` breaks JavaScript code.

---

## 11. Low: Miscellaneous Issues

### 11.1 "Its" vs "It's" — ~10 entries

| Entry | Output | Problem |
|-------|--------|---------|
| 11092 | `Its an emergency.` | Should be `It's` |
| 24143 | `Its great for q1.` | Should be `It's` |
| 26001 | `Its hard to estimate the work...` | Should be `It's` |

### 11.2 Unhyphenated Compounds — ~235 entries

| Pattern | Count | Example |
|---------|-------|---------|
| `follow up` (noun) | ~173 | `Schedule a follow up.` → should be `follow-up` |
| `non X` (adjective) | ~62 | `non normal data` → should be `non-normal` or `nonnormal` |

### 11.3 Double Period — 2 entries

| Entry | Output |
|-------|--------|
| 51005 | `...includes a force majeure clause..` |
| 53797 | `Did we achieve statistical significance? The p-value..?` |

### 11.4 "An" Before Consonant Sounds — ~37 entries

Most are actually correct ("an NDA" = "an en-dee-ay"), but some are wrong:

| Entry | Output | Problem |
|-------|--------|---------|
| 31293 | `This is an randomized controlled trial, phase two.` | Should be "a randomized" |
| 50849 | `We've been using an HTTP server...` | Correct (aitch-tee-tee-pee) |

### 11.5 Identical Input/Output — 41 entries

41 entries where the input is already clean and the output is identical. These are fine for training (model learns to pass through clean input), but could be reviewed to ensure there isn't a missed cleanup.

### 11.6 Spaced-Out Acronyms — 2 entries

| Entry | Output |
|-------|--------|
| 146 | `Can you clarify the I R R assumptions?` |
| 87836 | `We should review the I R R calculations.` |

Should be "IRR" in both.

---

## 12. Clean: Areas That Passed

- **Whitespace/encoding:** Zero BOM markers, zero-width spaces, double spaces, trailing spaces, tabs, smart quotes, em/en dashes, or non-breaking spaces. Spotless.
- **Filler removal:** The vast majority of "uh", "um", "er", "like you know" are correctly removed from outputs.
- **Apostrophes:** Consistent straight apostrophes throughout.
- **Technical term capitalization:** WebSocket, PostgreSQL, Kubernetes, GraphQL, PyTorch, TensorFlow, Elasticsearch, OAuth, HTTPS, etc. are generally well-capitalized (with the Go/MRR/US exceptions noted).
- **Self-correction handling:** The majority of "wait no X" / "scratch that X" patterns are handled correctly — the corrected version is kept. The failures noted in section 8.2 are a minority.
- **"Should of" → "should have":** Properly fixed throughout (the 6 "should of course" hits are legitimate uses of "of course").
- **"For all intensive purposes" → "for all intents and purposes":** Fixed correctly.
- **"Effect" vs "affect":** Fixed correctly (entry 49, sample).

---

## 13. Summary Table

| Severity | Issue | Count | Fixable? |
|----------|-------|-------|----------|
| **CRITICAL** | "X and I am" conjugation | 398 | Yes — regex replace |
| **CRITICAL** | "We've" as main verb | ~2,000 | Yes — pattern match + replace |
| **HIGH** | Missing comma before "but" | ~2,227 | Partially — needs NLP to detect compound sentences |
| **HIGH** | Missing comma after subordinate clause | ~411 | Yes — rule-based for sentence-initial clauses |
| **HIGH** | "The the" repeated | ~300 | Yes — regex replace |
| **HIGH** | Dictation artifacts ("question mark" literal) | ~15 clear | Partially — "period" is ambiguous |
| **MEDIUM** | Number format inconsistency (%, AM/PM, commas) | ~3,000+ | Yes — choose a standard and enforce |
| **MEDIUM** | Over-cleaning (hedges removed) | ~928 | Hard — requires human judgment |
| **MEDIUM** | Failed self-corrections | ~50–100 est. | Hard — requires semantic understanding |
| **MEDIUM** | Tone formalization (gonna→going to) | ~3,568 | Design decision |
| **MEDIUM** | Lowercase Go/MRR/US | ~75 | Yes — targeted regex |
| **LOW** | Unhyphenated compounds | ~235 | Yes — dictionary-based |
| **LOW** | Tag question missing comma | ~115 | Yes — regex for "X right?" |
| **LOW** | Its/It's confusion | ~10 | Manual |
| **LOW** | Subject-verb disagreement | ~8 | Manual |
| **LOW** | Broken grammar (for moment, like work, to doctor) | ~5 | Manual |
| **LOW** | Literal "slash" in output | ~20 clear | Partially — many are legitimate |
| **LOW** | Capital I in code | 5 | Yes — detect code context |
| **LOW** | Identical I/O | 41 | Harmless |

**Estimated total affected entries: ~7,000–8,000** (with overlap — some entries have 2–3 issues).

---

## 14. Recommendations

### Priority 1 — Fix Now (Mechanical, High Impact)

1. **Fix "and I am" → "and I are"** across all 398 entries. Simple regex: `/ and I am\b/ → / and I are/`.
2. **Fix "We've" as main verb.** Pattern: "We've" followed by article/number/adjective (not past participle). Replace with "We have".
3. **Fix "The the" → "The"** in all ~300 entries. Regex: `/\bThe the\b/i → The/` (case-insensitive for "the the").
4. **Fix "I R R" → "IRR"** in 2 entries.

### Priority 2 — Fix Now (Rule-Based, Medium Effort)

5. **Add commas after sentence-initial subordinate clauses.** Target patterns: `^(If|When|While|After|Before|Since|Although|Because|Unless|Once) .{15,}[^,] (the|we|you|it|I|they|he|she)`.
6. **Add commas before "right?"** at sentence end.
7. **Standardize AM/PM** — pick uppercase `AM`/`PM` (already dominant) and fix the 260 lowercase entries.
8. **Fix lowercase "go"** → "Go" when followed by service/worker/module/binary/code.
9. **Fix lowercase "mrr"** → "MRR" and "us" → "US" where contextually appropriate.

### Priority 3 — Design Decision Required

10. **Comma before "but":** Decide if this is a rule. If yes, it affects ~2,227 entries — significant but mechanically fixable.
11. **Percent format:** Pick `%` or "percent" and standardize. I'd recommend `%` — it's more common in the data and more natural in dictation output.
12. **Tone preservation:** Should "gonna" sometimes stay as "gonna"? Current behavior always formalizes. This is a product decision that affects how natural the output sounds.
13. **Hedging preservation:** "I think" and "probably" carry meaning. Consider keeping them unless they're part of a false start ("I think we should we need to...").

### Priority 4 — Hard Problems

14. **Failed self-corrections** (entry 72, 379, 436 etc.) require semantic understanding to fix. These are a small minority but the most damaging when they occur — they produce nonsense or the opposite of what the speaker intended.
15. **Dictation artifact "period"** is ambiguous when the word "period" is legitimately used. May need manual review of the ~290 entries.
