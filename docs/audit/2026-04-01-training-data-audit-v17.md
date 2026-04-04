# Training Data Audit: data_lfm_v17/train.jsonl

- **Version:** 1.0
- **Date:** 2026-04-01
- **Status:** Approved
- **Dataset:** `~/sotto-finetune/data_lfm_v17/train.jsonl` (on 192.168.1.128)
- **Total entries:** 148,021

## Table of Contents

1. [Executive Summary](#1-executive-summary)
2. [Dataset Overview](#2-dataset-overview)
3. [Critical Quality Issues](#3-critical-quality-issues)
4. [Consistency Issues](#4-consistency-issues)
5. [Incomplete Cleanup in Outputs](#5-incomplete-cleanup-in-outputs)
6. [Missing Use Cases](#6-missing-use-cases)
7. [Distribution Analysis](#7-distribution-analysis)
8. [Recommendations](#8-recommendations)

---

## 1. Executive Summary

The v17 training dataset contains 148,021 entries. While the overall quality is reasonable -- most entries demonstrate correct filler removal, punctuation, and capitalization -- the audit uncovered several significant issues that are likely degrading model performance:

| Issue Category | Entries Affected | % of Dataset |
|---|---:|---:|
| Redundant exact duplicates | 38,554 | 26.0% |
| Tech term capitalization errors | ~3,671 | 2.5% |
| Spoken punctuation not converted | ~1,380 | 0.9% |
| Day-of-week not capitalized | ~1,161 | 0.8% |
| Questions ending with `.` not `?` | ~1,008 | 0.7% |
| Missing apostrophes in output | ~847 | 0.6% |
| Filler words remaining in output | ~1,680+ | 1.1% |
| Corrupted output (`tI'me`) | 460 | 0.3% |
| Lowercase pronoun `i` | ~254 | 0.2% |

The **single largest issue** is the 38,554 redundant duplicate entries (26% of the dataset). Some individual pairs are repeated 100+ times, heavily skewing the model toward a small set of outputs. After deduplication, the effective dataset is ~109,467 unique pairs.

---

## 2. Dataset Overview

### 2.1 Length Distributions

| Metric | Input | Output |
|---|---:|---:|
| Min (chars) | 5 | 3 |
| Max (chars) | 4,695 | 4,232 |
| Mean (chars) | 91.8 | 74.3 |
| Median (chars) | 65.0 | 54.0 |
| Min (words) | 2 | 1 |
| Max (words) | 871 | 657 |
| Mean (words) | 17.2 | 13.2 |
| Median (words) | 12.0 | 10.0 |

### 2.2 Input Word Count Distribution

| Range | Count | % |
|---|---:|---:|
| 0-5 words | 1,166 | 0.8% |
| 5-10 words | 30,961 | 20.9% |
| 10-15 words | 64,284 | 43.4% |
| 15-20 words | 27,700 | 18.7% |
| 20-30 words | 18,689 | 12.6% |
| 30-50 words | 3,820 | 2.6% |
| 50-100 words | 451 | 0.3% |
| 100-200 words | 35 | 0.0% |
| 200+ words | 915 | 0.6% |

The dataset is heavily concentrated in the 5-20 word range (83% of entries). Long-form inputs (50+ words) represent only 1% but include 915 entries over 200 words.

### 2.3 Output/Input Ratio

| Ratio Range | Count | % | Meaning |
|---|---:|---:|---|
| 0.00-0.30 | 3,367 | 2.3% | Heavy reduction (self-correction collapse) |
| 0.30-0.50 | 16,628 | 11.2% | Significant filler removal |
| 0.50-0.70 | 27,076 | 18.3% | Moderate cleanup |
| 0.70-0.85 | 27,669 | 18.7% | Light cleanup |
| 0.85-1.00 | 35,802 | 24.2% | Minimal change (punctuation/caps) |
| 1.00-1.15 | 36,971 | 25.0% | Output longer (added punctuation/expansion) |
| 1.15+ | 508 | 0.3% | Rare |

Mean ratio: 0.792. Healthy distribution showing the model learns various levels of cleanup.

### 2.4 Domain Distribution

| Domain | Count | % |
|---|---:|---|
| Tech/programming | 28,281 | 19.1% |
| Meetings/work | 7,515 | 5.1% |
| Finance | 6,839 | 4.6% |
| Legal | 5,784 | 3.9% |
| Numbers/data | 5,596 | 3.8% |
| Medical | 4,995 | 3.4% |
| Email/messaging | 4,254 | 2.9% |

Tech/programming is the dominant domain at 19%. Other professional domains are represented but at much lower rates.

### 2.5 Primary Transformation Types

| Category | Count | % |
|---|---:|---:|
| Mixed/other | 57,814 | 39.1% |
| Filler removal | 38,717 | 26.2% |
| Punctuation only | 20,802 | 14.1% |
| List formatting | 9,605 | 6.5% |
| Spoken punctuation conversion | 9,028 | 6.1% |
| Grammar correction | 4,439 | 3.0% |
| Informal expansion (gonna->going to) | 3,560 | 2.4% |
| Heavy reduction | 3,367 | 2.3% |
| Corrupted output | 460 | 0.3% |
| Phonetic correction (jason->JSON) | 229 | 0.2% |

---

## 3. Critical Quality Issues

### 3.1 Corrupted Output: `tI'me` (460 entries)

All 460 entries share this exact pattern -- the output `"We need more tI'me."` instead of `"We need more time."` This is clearly a data generation bug.

**Inputs that produce this corruption** (all map to the same broken output):
- `"so basically what im getting at is we need more time"`
- `"what im trying to say is we need more time"`
- `"you know what i mean like we need more time"`
- `"honestly what im trying to get across is we need more time"`
- `"so like the thing is basically we need more time"`

Each of these 6 input variants appears ~77 times, always mapping to `"We need more tI'me."`. **These must be removed entirely.**

**Severity: Critical** -- these actively teach the model to produce corrupted text.

### 3.2 Massive Duplication (38,554 redundant entries)

Of 148,021 total entries, only **109,467 are unique** (input, output) pairs. 7,347 pairs appear more than once, with the worst offenders repeated 100+ times:

| Duplicates | Output |
|---:|---|
| 114x | `The build passed.` |
| 112x | `The code review found issues.` |
| 110x | `Tests green.` |
| 107x | `She and I were assigned to this project.` |
| 107x | `The ops team and I are investigating this week.` |
| 106x | `Approved.` |
| 106x | `He and I don't know the answer.` |
| 106x | `We need more developers.` |
| 105x | `We need more tI'me.` (corrupted!) |
| 105x | `She and I are going to fix this.` |

The top 30 most-common outputs all appear 300+ times each. This extreme repetition will cause the model to overfit toward these specific outputs.

There are also **2 inconsistent duplicate inputs** that map to different outputs:
1. `"okay uh so basically yeah its done"` -> `"It's done."` AND `"Its done."` (apostrophe inconsistency)
2. `"i want you to merge the pr after the tests pass"` -> `"...the PR..."` AND `"...the pr..."` (capitalization inconsistency)

**Severity: High** -- 26% of training signal is redundant and skews the model.

### 3.3 Missing Apostrophes in Output (847 entries)

Outputs that contain uncontracted forms like `dont`, `cant`, `weve`, `Lets`, `Im`, etc. where the apostrophe is missing:

| Pattern | Wrong | Correct | Error Rate |
|---|---:|---:|---:|
| `dont` / `don't` | 254 | 2,130 | 10.7% |
| `cant` / `can't` | 128 | 912 | 12.3% |
| `weve` / `we've` | 225 | 842 | 21.1% |
| `Lets` / `Let's` | 685 | 1,800 | 27.6% |
| `Im` / `I'm` | 297 | 3,032 | 8.9% |

Examples:
- `"So basically the API endpoint is down, and we cant retrieve the data from the FPGA board."`
- `"No the warranty is expired so we cant replace the broken screen."`
- `"Lets go."` (appears 552 times!)

**Severity: High** -- this creates conflicting training signal for apostrophe insertion.

---

## 4. Consistency Issues

### 4.1 Filler Word Treatment Is Inconsistent

The dataset sends **mixed signals** about when to keep vs. remove discourse markers:

| Word | In Input | Kept in Output | Removal Rate |
|---|---:|---:|---:|
| `um` | 28,668 | 147 | 99.5% |
| `uh` | 31,501 | 35 | 99.9% |
| `like` (all) | 11,652 | 3,756 | 67.8% |
| `you know` | 11,524 | 1,614 | 86.0% |
| `basically` | 18,404 | 3,043 | 83.5% |
| `actually` | 14,956 | 4,717 | 68.5% |
| `so` (start) | 12,014 | 3,436 | 71.4% |
| `honestly` | 4,158 | 292 | 93.0% |
| `right` (end) | 5,439 | 1,073 | 80.3% |
| `okay` (start) | 7,652 | 1,173 | 84.7% |
| `i mean` | 8,098 | 405 | 95.0% |

**The problem:** `um` and `uh` are removed 99.5-99.9% of the time, establishing a clear rule. But words like `basically`, `actually`, `you know`, and `like` are kept 15-32% of the time, **without a clear semantic distinction** for when they should stay. Some kept examples are clearly filler usage:

- `"So basically we can just meet at the park and then grab sandwiches."` (basically adds nothing)
- `"Like, the pipeline shows strong momentum, you know, basically."` (three fillers in one output)
- `"Actually no like the period is not over and wait like we need to check Q3 and Q4, you know."` (multiple fillers)

There are 1,063 outputs with **3 or more filler-type words** remaining.

**The um/uh exceptions are also questionable.** 147 outputs still contain `um` and 35 contain `uh`, often at sentence boundaries where they should have been removed:
- `"Um wait the trial period is already over right?"` (should be: `"Wait, the trial period is already over, right?"`)
- `"Um no that is not a bug it is expected behavior right."` (should be: `"No, that is not a bug; it is expected behavior, right?"`)

### 4.2 Question Mark Inconsistency

Questions are inconsistently punctuated. For outputs starting with question words:

| Pattern | Ends with `?` | Ends with `.` | Error Rate |
|---|---:|---:|---:|
| `Can you...` | 828 | 248 | 23.0% |
| `Is the...` | 454 | 240 | 34.6% |
| `Did you...` | 174 | 62 | 26.3% |
| `Should we...` | 61 | 22 | 26.5% |
| `Do we...` | 46 | 22 | 32.4% |

Examples of questions ending with `.` instead of `?`:
- `"Can you pause the interview process for the manager role until we fix the job description."`
- `"Did you check the jwt tokens they are expiring soon."`
- `"Could you rewrite it."`

**Severity: Medium** -- ~1,008 entries teach wrong punctuation for questions.

### 4.3 Tech Term Capitalization Errors (~3,671 entries)

Many tech terms are inconsistently capitalized in outputs:

| Term | Correct Cap | Wrong Cap | Error Rate |
|---|---:|---:|---:|
| `api` / `API` | 2,919 | 1,577 | 35.1% |
| `redis` / `Redis` | 840 | 300 | 26.3% |
| `docker` / `Docker` | 1,035 | 209 | 16.8% |
| `rust` / `Rust` | 712 | 185 | 20.6% |
| `nginx` / `Nginx` | 582 | 142 | 19.6% |
| `python` / `Python` | 590 | 119 | 16.8% |
| `react` / `React` | 616 | 101 | 14.1% |
| `graphql` / `GraphQL` | 694 | 102 | 12.8% |
| `kubernetes` / `Kubernetes` | 1,168 | 102 | 8.0% |
| `svelte` / `Svelte` | 560 | 94 | 14.4% |
| `elasticsearch` / `Elasticsearch` | 712 | 83 | 10.4% |
| `aws` / `AWS` | 49 | 63 | 56.3% |
| `typescript` / `TypeScript` | 587 | 71 | 10.8% |

`api` -> `API` has the worst consistency (35% error rate). `aws` is even worse at 56%.

Examples of wrong capitalization:
- `"The api is slow."` (appears as the #1 most common incorrect output)
- `"We should use redis."` 
- `"The react build is blocking the release."`

### 4.4 Day-of-Week Capitalization

`Monday` has the worst error rate among days:

| Day | Capitalized | Uncapitalized | Error Rate |
|---|---:|---:|---:|
| Monday | 475 | 1,025 | 68.3% |
| Friday | 1,230 | 185 | 13.1% |
| Tuesday | 232 | 79 | 25.4% |
| Wednesday | 378 | 32 | 7.8% |
| Thursday | 156 | 32 | 17.0% |
| Saturday | 138 | 15 | 9.8% |
| Sunday | 83 | 10 | 10.8% |

**`Monday` is uncapitalized in 68% of outputs.** This is likely a data generation bug specific to Monday. All other days are capitalized 75%+ of the time.

### 4.5 "Really Really" Kept 95% of the Time

Repeated intensifiers `"really really"` appear 755 times in inputs and are kept in 720 outputs (95.4%). This seems like an intentional design choice (preserve emphasis), but:
- It is inconsistent with the general approach of removing redundancy
- Some examples are clearly unnecessary: `"Really really important is the OKR alignment today."`

### 4.6 Number Word-to-Digit Conversion

Number words are **inconsistently** converted to digits:
- Converted to digits: 33.9% of cases
- Kept as words: 66.1% of cases

No clear rule distinguishes when `"twenty five"` becomes `"25"` vs stays as `"twenty five"`. Example contrast:
- `"running about twenty five minutes behind"` -> `"25 minutes behind"` (converted)
- `"IRR hit twenty percent"` -> `"IRR hit twenty percent"` (kept)

### 4.7 Informal Contraction Expansion

Informal contractions are **mostly** expanded, but not always:

| Contraction | Expanded | Kept | Expansion Rate |
|---|---:|---:|---:|
| `gonna` -> `going to` | 3,488 | 145 | 94.1% |
| `gotta` -> `got to/have to` | 1,168 | 112 | 86.0% |

The 3-14% of cases where they are kept sends mixed signals.

---

## 5. Incomplete Cleanup in Outputs

### 5.1 Spoken Punctuation Not Converted

While spoken punctuation (`period`, `comma`, `question mark`) is correctly converted in most cases, a significant number of entries fail:

| Spoken Word | Remaining in Output | Ambiguous (legit use) | Likely Errors |
|---|---:|---:|---:|
| `period` | 2,024 | ~830 (trial/grace period etc.) | ~1,195 |
| `comma` | 152 | ~30 (JSON comma) | ~120 |
| `question mark` | 33 | ~5 | ~28 |
| `exclamation point` | 20 | 0 | ~20 |

Examples of failures:
- IN: `"my main topic is green energy slash renewable power comma and i need two examples period"`
  OUT: `"My main topic is green energy slash renewable power comma and I need two examples period."` (NOTHING converted!)
- IN: `"the findings are significant question mark but we must verify the outliers period"`
  OUT: `"The findings are significant question mark but we must verify the outliers period."` (NOTHING converted!)

Some entries have spoken punctuation mixed with legitimate use of the same word, creating ambiguity:
- IN: `"wait for the build period"` -- does "period" mean the punctuation mark or "the build period" (time span)?

### 5.2 Slash Handling Inconsistency

The spoken word `"slash"` is handled three different ways:
- Converted to `/`: 64.3%
- Kept as word `slash`: 10.3%
- Sometimes removed entirely or converted to a comma: 25.4%

### 5.3 Outputs Missing Terminal Punctuation (8,871 entries)

6.0% of outputs end without terminal punctuation (period, question mark, or exclamation). Nearly all of these are **list-formatted outputs** where the last numbered item doesn't end with a period. This is a systematic formatting choice, but it is inconsistent -- some lists do end with punctuation and others don't.

### 5.4 Lowercase Pronoun `i` (254 entries)

254 outputs contain lowercase `i` as a pronoun:
- `"Like i said we prefer graphql over rest for the api endpoints"`
- `"No actually the surgery plan needs a revision i like the new timeline"`

---

## 6. Missing Use Cases

### 6.1 Severely Underrepresented

| Use Case | Count | Notes |
|---|---:|---|
| **Dollar amounts** (`$50`, `$1,000`) | 1 | Almost zero financial formatting |
| **Percentages** (`15%`) | 18 | Numbers with `%` symbol are nearly absent |
| **Phone numbers** | 2 | |
| **Email addresses** | 2 | |
| **Code dictation** (parentheses, brackets, etc.) | 42 | |
| **Multi-speaker** scenarios | 58 | |
| **Accent/dialect patterns** | <30 total | Almost zero y'all, innit, mate, etc. |
| **URLs** in text | 118 | |
| **snake_case identifiers** | 10 | |
| **camelCase identifiers** | 0 | |
| **Function call syntax** | 0 | |

### 6.2 Notably Absent

- **Numeric dictation**: `"one two three four five six seven eight nine zero"` as a phone number, serial number, or credit card
- **Spelling out words**: `"B as in bravo, O as in oscar, B as in bravo"` -> `"BOB"`
- **Email dictation**: `"john at example dot com"` -> `"john@example.com"`
- **Mixed language**: Code-switching or borrowed terms from other languages
- **Timestamps**: `"two thirty PM"` -> `"2:30 PM"`
- **Abbreviation expansion**: `"e g"` -> `"e.g."`, `"i e"` -> `"i.e."`, `"etc"` -> `"etc."`
- **Mathematical expressions**: `"x squared plus 2 x minus 5"`
- **Hashtags/mentions**: `"at username"` -> `"@username"`, `"hashtag trending"` -> `"#trending"`

### 6.3 Underrepresented Domains

- **Casual/personal conversation** is present but light compared to tech/professional
- **Customer service / support** interactions
- **Education / teaching** contexts
- **Creative writing** dictation
- **Sports / entertainment** jargon

---

## 7. Distribution Analysis

### 7.1 Duplicate Impact on Output Distribution

The top 30 most-common outputs account for approximately 12,000 entries (8% of dataset). After removing duplicates beyond 1 occurrence, the dataset would shrink from 148,021 to 109,467 entries (26% reduction).

### 7.2 Input Length Skew

83% of inputs are 5-20 words. Real-world ASR segments can easily be 50-200 words (a full spoken paragraph). Only 0.9% of inputs exceed 50 words. This may cause the model to struggle with longer inputs in production.

### 7.3 Preserve-Wording Representation

31.6% of entries (46,795) have an output/input ratio > 0.95, meaning the output is nearly the same length as the input. This is a healthy representation of "light touch" cleanup cases. However, **zero entries** have identical input and output, meaning there are no explicit "pass-through" examples where the input needs no changes at all. This could cause the model to always make some modification, even when the input is already clean.

### 7.4 Phonetic ASR Correction

Only a handful of phonetic corrections exist:
- `jason` -> `JSON`: 337 entries
- `post gres` -> `PostgreSQL`: 596 entries (using postgres, not the phonetic form)
- `nine jex` -> `Nginx`: 1 entry
- `gee dee pee are` -> `GDPR`: 4 entries

Real ASR output would produce many more phonetic spellings of tech terms.

---

## 8. Recommendations

### 8.1 Immediate Fixes (data cleaning)

1. **Remove all 460 `tI'me` corrupted entries.** Search for `tI'm` in outputs and delete.

2. **Deduplicate to at most 2-3 copies per unique pair.** Remove 38,000+ redundant entries. Consider keeping 1 copy of each unique pair.

3. **Fix missing apostrophes in outputs.** Run a pass converting `dont` -> `don't`, `cant` -> `can't`, `weve` -> `we've`, `Lets` -> `Let's`, `Im` -> `I'm`, `Ill` -> `I'll`, etc. across all outputs.

4. **Fix `Monday` capitalization.** 1,025 outputs have uncapitalized `monday` -- fix to `Monday`. Check other day names as well.

5. **Fix tech term capitalization.** Particularly `api` -> `API` (1,577 errors), `redis` -> `Redis` (300 errors), `aws` -> `AWS` (63 errors). Build a mapping and apply it.

6. **Fix lowercase pronoun `i`.** 254 entries need `i` -> `I` correction.

7. **Fix questions ending with `.` instead of `?`.** ~1,008 entries need the terminal period changed to a question mark.

### 8.2 Consistency Improvements (policy decisions + re-generation)

8. **Establish clear filler word policy.** Decide whether `basically`, `actually`, `you know`, and `like` should be removed when used as fillers (not as content words). Currently the signal is too noisy (15-32% kept rate). Recommendation: remove filler usage, keep content usage (e.g., "I like the design" keeps "like"; ", like, the thing is" removes "like").

9. **Establish spoken punctuation disambiguation.** The word `period` is ambiguous (spoken command vs. "trial period"). Add context-aware handling or consistent rules.

10. **Decide on number word-to-digit policy.** Currently 34% converted, 66% kept as words. Recommendation: always convert context-dependent (keep "twenty-one" in casual, convert in technical/financial).

11. **Fix `really really` handling.** Currently kept 95% of the time. Either always keep it (intentional emphasis) or always reduce to single "really" -- do not send mixed signals.

12. **Standardize slash handling.** Decide when `slash` becomes `/` vs. stays as a word.

### 8.3 Data Augmentation (new entries needed)

13. **Add pass-through examples.** Create entries where the input is already clean and the output is identical. Currently zero such examples exist.

14. **Add dollar amounts and percentages.** Currently 1 and 18 entries respectively.

15. **Add email/URL dictation.** `"john at example dot com"` -> `"john@example.com"`.

16. **Add numeric dictation.** Phone numbers, serial numbers, timestamps.

17. **Add code dictation.** `"function open paren x close paren"` -> `"function(x)"`.

18. **Add longer inputs** (50-200 words). Currently only 0.9% of data covers this range.

19. **Add accent/dialect examples.** Currently almost none (1 `y'all`, 21 `ain't`, 0 `innit`).

20. **Add multi-speaker scenarios.** Currently only 58 entries.

### 8.4 Priority Order

| Priority | Action | Impact | Effort |
|---|---|---|---|
| P0 | Remove `tI'me` corruption (#1) | Eliminates active harm | Trivial |
| P0 | Deduplicate (#2) | Fixes 26% data skew | Low |
| P1 | Fix apostrophes (#3) | Fixes conflicting signal | Low |
| P1 | Fix Monday/tech caps (#4, #5) | Fixes capitalization signal | Low |
| P1 | Fix question marks (#7) | Fixes punctuation signal | Low |
| P2 | Filler policy + regen (#8) | Major consistency improvement | Medium |
| P2 | Pass-through examples (#13) | Prevents over-editing | Medium |
| P3 | Financial formatting (#14) | Fills gap | Medium |
| P3 | Email/URL/code dictation (#15-17) | Fills gaps | Medium-High |
| P3 | Longer inputs (#18) | Improves real-world performance | Medium |

---

## Appendix: Methodology

Analysis performed by SSH into `juanqui@192.168.1.128` and running Python scripts against the full 148,021-entry JSONL file. All statistics are exact counts (not sampled estimates). Random samples were drawn with fixed seeds for reproducibility. Filler word detection used `\b` word-boundary regex matching. Domain classification used keyword pattern matching (entries may match multiple categories).
