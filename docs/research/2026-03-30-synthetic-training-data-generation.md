# Synthetic Training Data Generation for Transcript Cleanup Fine-Tuning

- **Version:** 1.0
- **Date:** 2026-03-30
- **Status:** Superseded
- **Superseded by:** [2026-03-31-synthetic-training-data-generation-v2.md](../research/2026-03-31-synthetic-training-data-generation-v2.md)

## Table of Contents

1. [Summary](#1-summary)
2. [Problem Statement](#2-problem-statement)
3. [Data Requirements](#3-data-requirements)
4. [Disfluency Taxonomy](#4-disfluency-taxonomy)
5. [Domain Coverage](#5-domain-coverage)
6. [Generation Pipeline](#6-generation-pipeline)
7. [Prompt Templates](#7-prompt-templates)
8. [Quality Assurance](#8-quality-assurance)
9. [Dataset Composition](#9-dataset-composition)
10. [Output Format](#10-output-format)
11. [Validation Strategy](#11-validation-strategy)
12. [Cost Estimate](#12-cost-estimate)
13. [Risks and Mitigations](#13-risks-and-mitigations)

---

## 1. Summary

This document describes how to generate a high-quality synthetic training dataset for fine-tuning SmolLM2-135M (base) on the task of speech-to-text transcript cleanup. The goal is to produce 20,000–30,000 diverse (raw, cleaned) pairs using Claude as the teacher model. The resulting model must handle filler removal, self-correction, crutch words, dictation commands, grammar, misheard terms, list formatting, and false starts — while preserving the speaker's intentional wording.

## 2. Problem Statement

Our current 2B prompted model (Qwen3.5-2B-OptiQ-4bit) achieves ROUGE-L 0.891 with 16 cycles of prompt tuning. Key weaknesses remain:

| Category | ROUGE-L | Root cause |
|----------|---------|------------|
| self_correction | 0.742 | 2B model can't learn correction patterns from instructions |
| crutch_words | 0.879 | Instruction conflicts cause inconsistent removal |
| list_formatting | 0.859 | Prompted model unreliable at format changes |

A fine-tuned 135M model should learn these patterns directly from examples, eliminating prompt engineering fragility. But it has no pre-trained instruction-following ability — everything it knows comes from the training data. The data must therefore be:

- **Exhaustive in pattern coverage** — every disfluency type the model will encounter
- **Diverse in domain** — tech, medical, legal, casual, academic, etc.
- **Varied in difficulty** — from single-filler short phrases to 200+ word passages with overlapping issues
- **Precise in expected output** — the cleaned version must be exactly what we'd want, no more, no less

## 3. Data Requirements

### Target Volume

| Phase | Samples | Purpose |
|-------|---------|---------|
| Training set | 20,000 | Core supervised fine-tuning |
| Validation set | 2,000 | Loss monitoring during training |
| Test set | 500 | Final evaluation (held-out, never seen during training) |
| Benchmark set | 135 | Existing hand-curated set for regression tracking |
| **Total** | **~22,500** | |

**Rationale:** Research on fine-tuning small models (<500M params) for text-to-text tasks indicates 5K–20K examples as the productive range, with diminishing returns beyond 50K for narrow tasks (Scale Labs, 2024; Oumi research, 2025). We target 20K training samples to comfortably cover our 12 transformation categories across multiple domains and difficulty levels. The 135-sample hand-curated benchmark remains the primary regression test — it is never included in training or validation.

### Per-Sample Structure

Each sample is a pair:

| Field | Description |
|-------|-------------|
| `raw` | Simulated speech-to-text output with disfluencies injected |
| `clean` | The ideal cleaned version |
| `category` | Primary disfluency type (for stratified analysis) |
| `domain` | Content domain (tech, medical, casual, etc.) |
| `difficulty` | easy / medium / hard |
| `word_count` | Word count of `raw` field |
| `disfluency_tags` | List of disfluency types present (e.g., `["filler", "self_correction"]`) |

## 4. Disfluency Taxonomy

Based on linguistic classification of speech disfluencies (Shriberg, 1996; Wikipedia/Speech_disfluency), our training data must cover these categories:

### 4.1 Fillers (Non-Lexical Vocalizations)

Hesitation markers with no semantic content.

| Filler | Examples in context |
|--------|-------------------|
| uh, um, uhm | "I uh need the report" |
| er, ah | "The er connection timed out" |
| hmm | "Hmm I think we should wait" |

**Training approach:** Inject fillers at random positions — sentence-initial, mid-clause, between clauses, adjacent to other fillers. Vary density from 1 filler per sentence (light) to 4+ per sentence (heavy).

### 4.2 Crutch Words / Discourse Markers

Words that serve a conversational function but add no content when transcribed.

| Word/Phrase | Function | Remove? |
|-------------|----------|---------|
| basically | hedging | Yes |
| you know | rapport/hedging | Yes |
| I mean | reformulation | Yes |
| honestly | emphasis filler | Yes |
| literally | intensifier filler | Yes |
| anyway | topic shift filler | Yes |
| like | quotative/filler | Context-dependent |
| so | connective/filler | Context-dependent |
| okay | acknowledgment | Context-dependent |
| right | tag question | Context-dependent |
| yeah | agreement filler | Context-dependent |

**Training approach:** Generate examples where context-dependent words appear both as fillers (remove) and as meaningful words (keep). E.g., "I like this design" (keep) vs "It's like really slow" (remove "like"). This teaches the model contextual judgment.

### 4.3 Self-Corrections / Repairs

The speaker changes their mind mid-sentence. Correction is signaled by markers.

| Marker | Pattern | Example |
|--------|---------|---------|
| wait | "X, wait, Y" | "Send to dev, wait, send to staging" |
| actually | "X, actually Y" | "Use Python, actually use Rust" |
| no | "X, no, Y" | "Tuesday, no, Wednesday" |
| scratch that | "X, scratch that, Y" | "Add a spinner, scratch that, add a skeleton" |
| sorry | "X, sorry, Y" | "At two, sorry, two thirty" |
| I mean | "X, I mean Y" | "The function, I mean the method" |
| or rather | "X, or rather Y" | "A hundred, or rather a thousand users" |

**Training approach:** This is our weakest category (0.742 ROUGE-L on the prompted model). Generate high volume here — at least 3,000 self-correction examples. Include:
- Single corrections ("X, actually Y")
- Double corrections ("X, no Y, actually Z" — keep only Z)
- Corrections with reasoning ("X, wait that's too low, Y")
- Corrections where X and Y share partial structure ("Set font to 12, no, 14 pixels" → "Set font to 14 pixels.")
- Corrections that reassign tasks or change targets

### 4.4 False Starts / Abandoned Utterances

The speaker begins a thought, abandons it, and restarts.

| Pattern | Example |
|---------|---------|
| Word repetition | "The the server needs a restart" |
| Phrase restart | "We need to we should probably add tests" |
| Synonym restart | "The function the method takes two args" |
| Abandoned + new | "I was going to — let's just merge it" |

**Training approach:** Generate examples where the restart is both obvious (same word repeated) and subtle (synonym replacement, slight rephrase). Include cases where the false start shares no words with the continuation.

### 4.5 Grammar Errors (Spoken Vernacular)

Errors that occur naturally in spontaneous speech.

| Error type | Example | Correction |
|------------|---------|------------|
| gonna/wanna/gotta | "We gonna need more time" | "We're going to need more time" |
| Subject-verb agreement | "The tests is failing" | "The tests are failing" |
| Pronoun case | "Me and him will fix it" | "He and I will fix it" |
| Run-on sentences | "Fix the bug its blocking QA" | "Fix the bug. It's blocking QA." |
| Missing articles | "Send report to manager" | "Send the report to the manager" |
| its/it's confusion | "Its not working" | "It's not working" |
| should of | "We should of tested this" | "We should have tested this" |

**Training approach:** Mix grammar errors with other disfluency types in medium/hard examples. Include cases where spoken vernacular is idiomatic and should be left alone (e.g., "gonna" in a casual context might be acceptable depending on user preference — but our model should consistently correct it).

### 4.6 Misheard / ASR Errors

The speech-to-text engine produces phonetically plausible but semantically wrong text.

| Domain | ASR output | Correct |
|--------|-----------|---------|
| Tech | "oh auth two" | "OAuth 2.0" |
| Tech | "post gress" | "Postgres" |
| Tech | "Kuber Netties" | "Kubernetes" |
| Tech | "Jason" / "Jason payload" | "JSON" / "JSON payload" |
| Tech | "git hub" | "GitHub" |
| Medical | "hyper tension" | "hypertension" |
| Medical | "anna phylaxis" | "anaphylaxis" |
| Legal | "habeus corpus" | "habeas corpus" |
| General | "for all intensive purposes" | "for all intents and purposes" |
| General | "a whole nother" | "a whole other" |

**Training approach:** Generate domain-specific ASR errors. Use phonetic similarity to create plausible mishearings. Include both compound word splits ("web socket" → "WebSocket") and phonetic substitutions ("Quentin" → "Qwen"). Create examples where the surrounding context makes the correct term unambiguous.

### 4.7 Dictation Commands

Spoken punctuation and formatting commands (iOS-style).

| Spoken | Output |
|--------|--------|
| period | . |
| dot | . (especially in URLs/emails) |
| comma | , |
| slash | / |
| question mark | ? |
| exclamation point / exclamation mark | ! |
| colon | : |
| semicolon | ; |
| dash / hyphen | - |
| open parenthesis / close parenthesis | ( / ) |
| new line | \n |
| new paragraph | \n\n |
| quote / unquote / end quote | " |

**Training approach:** Generate examples with dictation commands in natural positions. Critically, also generate adversarial examples where these words appear as regular nouns — "the Jurassic period was long" (don't convert), "end the sentence period" (convert). The model must learn contextual disambiguation.

### 4.8 List Formatting

Spoken numbered items that should be formatted as lists.

| Spoken pattern | Output format |
|----------------|--------------|
| "first X second Y third Z" | 1. X\n2. Y\n3. Z |
| "one X two Y three Z" | 1. X\n2. Y\n3. Z |
| "step one X step two Y" | 1. X\n2. Y |
| "number one X number two Y" | 1. X\n2. Y |

**Training approach:** Include lists of 2–7 items. Include examples where number words appear in non-list contexts ("I have one concern" should NOT become a list). Include lists embedded in larger passages.

### 4.9 Preserve Wording (Identity Examples)

Clean or near-clean inputs where the model should make minimal or no changes.

**Training approach:** 15–20% of the dataset should be "identity" or "near-identity" examples — well-formed sentences that need only punctuation/capitalization. This prevents the model from developing a bias toward always changing text. Include:
- Perfectly clean sentences → output identical (plus punctuation)
- Sentences with emphasis words (really, very, definitely) → preserve exactly
- Sentences with intentional phrases ("go ahead and", "a lot of", "kind of") → preserve exactly
- Short clean commands ("Ship it", "Merge the PR") → preserve exactly

## 5. Domain Coverage

The model must generalize across speech domains. Training data is stratified across these domains:

| Domain | % of dataset | Rationale |
|--------|-------------|-----------|
| **Software engineering** | 25% | Primary user base |
| **General business** | 20% | Meetings, emails, status updates |
| **Casual/conversational** | 15% | Slack messages, quick notes |
| **Technical (non-software)** | 10% | Hardware, networking, data science |
| **Medical/health** | 8% | Clinical notes, patient instructions |
| **Legal/compliance** | 7% | Contracts, policies, regulations |
| **Academic/research** | 5% | Papers, lectures, presentations |
| **Creative/content** | 5% | Writing, marketing, social media |
| **Finance** | 5% | Reports, trading, budgets |

**Within each domain**, generate examples that use domain-specific vocabulary, acronyms, and conventions. The software engineering domain should include common frameworks, tools, and workflows. Medical should include drug names, procedures, and anatomical terms. Legal should include case citations and statutory language.

### Speaker Personas

To increase diversity, the teacher model generates examples from varied speaker perspectives:

| Persona | Speech characteristics |
|---------|----------------------|
| Senior engineer | Confident, technical jargon, concise |
| Junior developer | Hesitant, more fillers, seeks confirmation ("right?") |
| Manager | Strategic language, action items, delegating |
| Non-native English speaker | Occasional article/preposition errors, simpler vocabulary |
| Fast talker | Many false starts, corrections, incomplete thoughts |
| Deliberate speaker | Few fillers, longer pauses (represented as "um"), careful word choice |
| Domain expert | Heavy jargon, acronyms, assumed knowledge |
| Casual dictator | Short bursts, lots of dictation commands, informal |

## 6. Generation Pipeline

### Architecture

```
┌─────────────┐     ┌──────────────┐     ┌──────────────┐     ┌─────────────┐
│ Seed Config  │────▶│ Teacher LLM  │────▶│ Validator    │────▶│ Deduplicate │
│ (domain,     │     │ (Claude)     │     │ (rule-based  │     │ (cosine sim │
│  category,   │     │              │     │  + LLM judge)│     │  > 0.92)    │
│  difficulty,  │     │ Generates    │     │              │     │             │
│  persona)    │     │ (raw, clean) │     │ Filters bad  │     │ Removes     │
│              │     │ pairs        │     │ samples      │     │ near-dupes  │
└─────────────┘     └──────────────┘     └──────────────┘     └─────────────┘
                                                                     │
                                                                     ▼
                                                              ┌─────────────┐
                                                              │ Final       │
                                                              │ Dataset     │
                                                              │ (JSONL)     │
                                                              └─────────────┘
```

### Step 1: Seed Configuration Matrix

Generate a configuration matrix by crossing:
- **10 generation categories** × **9 domains** × **3 difficulties** × **8 personas** = 2,160 unique cells

(Note: the benchmark has 12 evaluation categories, but "short" and "long_dictation" are length-based slices, not distinct generation categories. Length variety is controlled by the difficulty axis.)

Not every cell needs examples (some combinations are nonsensical — e.g., casual persona + legal domain is rare). Target coverage: ~60% of cells with 5–15 examples each.

### Step 2: Batch Generation

For each batch, send a request to Claude with:
- The seed configuration (domain, category, difficulty, persona)
- 2–3 anchor examples showing the expected (raw, clean) format
- A request for 10 examples per batch
- Structured JSON output format

Use **temperature 0.9** for maximum lexical diversity across batches.

### Step 3: Rule-Based Validation

Automated checks on each generated pair:

| Check | Rule | Action on failure |
|-------|------|-------------------|
| Length ratio | 0.2 < len(clean)/len(raw) < 1.1 | Reject |
| No hallucination | Content words in `clean` must trace back to `raw` (fuzzy match, allowing grammar inflections and dictation symbol conversions) | Reject |
| Filler residual | No fillers (uh, um, basically, etc.) remain in `clean` | Reject |
| Format compliance | `clean` has proper punctuation, capitalization | Flag for review |
| Category match | Disfluency types in `raw` match declared category | Flag for review |
| Non-empty | Both fields have ≥ 3 words | Reject |
| Self-correction check | If category is self_correction, verify `clean` is shorter than `raw` | Flag for review |
| Identity check | If raw ≈ clean (cosine > 0.98), verify this is intentional (preserve_wording) | Flag for review |

### Step 4: LLM Judge (Second Pass)

Run a random 20% sample through a different LLM (or Claude with a judge prompt) to score:
- **Faithfulness (1–5):** Does `clean` preserve all meaningful content from `raw`?
- **Completeness (1–5):** Are all disfluencies properly handled?
- **Naturalness (1–5):** Does `clean` read like something a human would write?

Reject samples scoring < 3 on any dimension. This catches subtle errors that rule-based checks miss (e.g., paraphrasing that changes meaning).

### Step 5: Deduplication

Compute sentence embeddings (e.g., using `sentence-transformers/all-MiniLM-L6-v2`) and remove pairs where cosine similarity of `raw` fields exceeds 0.92. This prevents the model from memorizing specific phrasings.

### Step 6: Stratified Sampling

Ensure the final dataset matches the target composition (Section 9). Over-generate by 40% (target 28,000 raw pairs to yield ~20,000 after filtering).

## 7. Prompt Templates

### Primary Generation Prompt

```
You are generating training data for a speech-to-text transcript cleanup model.

Generate exactly 10 pairs of (raw_transcript, cleaned_transcript) for the
following configuration:

- Domain: {domain}
- Disfluency type: {category}
- Difficulty: {difficulty}
- Speaker persona: {persona}

Rules for the RAW transcript:
- Write it as if an ASR system transcribed spontaneous speech
- Include the specified disfluency type naturally
- Do NOT include punctuation in raw (ASR output rarely has punctuation)
- Use lowercase throughout (simulating raw ASR output)
- For {difficulty}:
  - easy: 1 disfluency instance, 10-25 words
  - medium: 2-3 disfluency instances, 20-60 words, may mix types
  - hard: 4+ disfluency instances, 40-150 words, multiple overlapping types

Rules for the CLEANED transcript:
- Remove ONLY verbal fillers, crutch words, stuttered repetitions, and
  false starts
- Fix punctuation, capitalization, and grammar
- Convert spoken punctuation ("period" → ".", "comma" → ",", etc.)
- For self-corrections: keep ONLY the final/corrected version
- Format numbered items as numbered lists
- Do NOT paraphrase, summarize, reword, or restructure
- Preserve emphasis words (really, very, definitely) and intentional
  phrases ("go ahead and", "I want you to", "a lot of")
- The cleaned version should be what a careful human transcriptionist
  would produce

Output as JSON array:
[
  {
    "raw": "the uh server is uh running low on memory",
    "clean": "The server is running low on memory.",
    "word_count": 9,
    "disfluency_tags": ["filler"]
  },
  ...
]
```

### Adversarial / Edge Case Prompt

```
Generate 10 TRICKY pairs where a naive cleanup model would make mistakes.
Focus on cases where:
- A word that is usually a filler is used meaningfully
  (e.g., "I like this" — don't remove "like")
- A self-correction marker is used literally
  (e.g., "click the Actually button" — don't treat as correction)
- Dictation command words appear as regular nouns
  (e.g., "the Jurassic period" — don't convert to ".")
- The speaker uses emphasis that looks like it could be removed
  (e.g., "I really really need this" — the repetition is intentional emphasis)
- Clean input that needs no changes (identity function)

Domain: {domain}
```

### Long-Form Dictation Prompt

```
Generate 5 long-form dictation examples (100-250 words each) that simulate
a user dictating into SottoASR. The speaker is a {persona} working in
{domain}.

The raw transcript should include:
- Natural speech disfluencies scattered throughout (3-8 instances)
- At least one self-correction
- At least one dictation command (period, comma, etc.)
- Run-on sentences (no punctuation in raw)
- Domain-specific terminology

The cleaned version should preserve all meaningful content while cleaning
only the disfluencies. Total length should be within 90-100% of the raw
length (after removing fillers).
```

## 8. Quality Assurance

### The Contamination Problem

The biggest risk in synthetic data is **distributional narrowness** — the teacher model's biases become the student model's blind spots. Mitigations:

1. **Temperature 0.9** for generation (maximize diversity)
2. **Persona rotation** ensures varied speech styles
3. **Domain rotation** ensures varied vocabulary
4. **Anchor examples rotated** every 50 batches to prevent pattern lock-in
5. **Manual spot-check:** Human review of 200 random samples before training
6. **Held-out test set** from our existing 135-sample benchmark — never included in training, never generated by the same prompt template

### Quality Metrics on Generated Data

Before training, measure these on the full generated dataset:

| Metric | Target | How to measure |
|--------|--------|----------------|
| Unique raw trigrams | > 80% unique | Count distinct trigrams across all raw samples |
| Domain balance | Within 3% of target distribution | Count per domain |
| Category balance | Within 5% of target distribution | Count per category |
| Avg length ratio (clean/raw) | 0.75–0.95 | Mean across dataset |
| Filler residual rate | 0% | Regex scan of all `clean` fields |
| Near-duplicate rate | < 2% | Cosine similarity clustering |
| LLM judge pass rate | > 85% | Sample 2,000 examples, score with judge |

## 9. Dataset Composition

### By Category

| Category | % of dataset | ~Count | Notes |
|----------|-------------|--------|-------|
| filler_removal | 12% | 2,400 | High variety in filler density and position |
| self_correction | 15% | 3,000 | Highest volume — our weakest area |
| crutch_words | 10% | 2,000 | Include context-dependent examples |
| false_start | 8% | 1,600 | Range from simple repetition to full restarts |
| grammar | 8% | 1,600 | Spoken vernacular corrections |
| misheard_words | 8% | 1,600 | Domain-specific ASR errors |
| dictation_commands | 8% | 1,600 | Include adversarial non-command cases |
| list_formatting | 6% | 1,200 | 2–7 item lists in varied contexts |
| preserve_wording | 15% | 3,000 | Identity/near-identity — prevents over-cleaning |
| mixed (multi-type) | 10% | 2,000 | Overlapping disfluency types in one passage |

### By Difficulty

| Difficulty | % of dataset | Description |
|------------|-------------|-------------|
| Easy | 30% | Single disfluency type, short (10–25 words) |
| Medium | 45% | 2–3 types, medium length (20–60 words) |
| Hard | 25% | 4+ types, long (40–250 words), overlapping issues |

### By Length

| Length bucket | % of dataset | Word count |
|---------------|-------------|------------|
| Short | 25% | 5–15 words |
| Medium | 40% | 15–50 words |
| Long | 25% | 50–150 words |
| Very long | 10% | 150–250 words |

## 10. Output Format

### Training Format (JSONL)

Each line is a JSON object with `input` and `output` fields for sequence-to-sequence fine-tuning:

```jsonl
{"input": "the uh server is uh running low on memory", "output": "The server is running low on memory."}
{"input": "use python actually no lets use rust for this", "output": "Let's use Rust for this."}
{"input": "ship it", "output": "Ship it."}
```

No system prompt, no chat template, no role fields. The model learns a pure text-to-text mapping. This is intentional — SmolLM2-135M base has no chat template, and adding one would waste tokens.

**Important: SmolLM2-135M is a decoder-only model**, not encoder-decoder like T5. The training script must concatenate input and output with a separator token so the model learns to generate the output given the input. A typical format for the training loop:

```
<s>INPUT: the uh server is uh running low on memory
OUTPUT: The server is running low on memory.</s>
```

The `input`/`output` JSONL format above is an interchange format. The training script transforms each pair into the model-specific concatenated sequence, with the loss computed only on the `OUTPUT:` portion.

### Metadata Sidecar (for analysis, not training)

A separate `metadata.jsonl` file with per-sample metadata:

```jsonl
{"id": "train_00001", "category": "filler_removal", "domain": "software_engineering", "difficulty": "easy", "persona": "senior_engineer", "word_count": 9, "disfluency_tags": ["filler"]}
```

This enables stratified evaluation without polluting the training data.

## 11. Validation Strategy

### Pre-Training Validation

1. **Parse check:** Every JSONL line parses as valid JSON with `input` and `output` keys
2. **Rule-based filters:** Apply all checks from Section 6 Step 3
3. **Distribution audit:** Verify category/domain/difficulty ratios match Section 9
4. **Manual review:** Human reads 200 random samples, flags systematic issues
5. **Benchmark overlap check:** Verify zero overlap between training data and the 135-sample benchmark set (cosine similarity < 0.85 against all benchmark `raw` fields)

### Post-Training Validation

1. **Benchmark regression:** Run the 135-sample benchmark — primary quality gate
2. **Category breakdown:** Verify improvement on self_correction (target > 0.85 ROUGE-L)
3. **Domain generalization:** Test on 50 held-out examples from each domain
4. **Adversarial robustness:** Test on 50 adversarial examples (filler words used meaningfully)
5. **Length generalization:** Test on 20 examples of 200+ words
6. **Identity preservation:** Test on 50 clean inputs — output should be identical or near-identical

## 12. Cost Estimate

### Generation Costs (Claude API)

| Item | Calculation | Cost |
|------|------------|------|
| Batches needed | 28,000 samples ÷ 10 per batch = 2,800 batches | — |
| Avg input tokens per batch | ~800 (prompt + config) | — |
| Avg output tokens per batch | ~1,500 (10 JSON examples) | — |
| Total input tokens | 2,800 × 800 = 2.24M | ~$6.72 (at $3/1M) |
| Total output tokens | 2,800 × 1,500 = 4.2M | ~$63.00 (at $15/1M) |
| **Generation subtotal** | | **~$70** |
| LLM judge (20% sample) | 560 batches × ~500 tokens = 280K tokens | ~$5 |
| **Total** | | **~$75** |

### Compute Costs (Fine-Tuning)

| Item | Estimate |
|------|----------|
| Model | SmolLM2-135M (base) |
| Method | Full fine-tune or LoRA (rank 16–64) |
| Hardware | Single A10 GPU or Mac M-series with 16GB+ RAM |
| Training time | ~2–4 hours for 20K samples, 3 epochs |
| Cloud cost | ~$5–10 (if using cloud GPU) |

**Total estimated cost: ~$80–85.**

## 13. Risks and Mitigations

| Risk | Impact | Likelihood | Mitigation |
|------|--------|------------|------------|
| **Teacher model bias** — Claude produces stylistically narrow text | Model learns Claude's writing style, not diverse speech patterns | Medium | High temperature, persona rotation, domain diversity, manual spot-checks |
| **Distributional mismatch** — synthetic speech doesn't match real ASR output | Model fails on real transcriptions | Medium | Include 200+ real SottoASR transcriptions in training set when available; validate against real-world examples in benchmark |
| **Over-correction bias** — model trained on too many "fix this" examples starts changing clean text | Model edits text that needs no editing | High | 15% of dataset is preserve_wording (identity examples); test with clean-input evaluation set |
| **Domain blind spots** — underrepresented domains fail | Medical/legal users get poor results | Low | Stratified domain coverage, held-out per-domain test sets |
| **Catastrophic forgetting** — fine-tuning destroys base model capabilities | Model produces garbled output on edge cases | Low | Use LoRA to preserve base weights; validate on diverse held-out set |
| **Adversarial failures** — "period" converted in "Jurassic period" | Embarrassing errors on common words | Medium | Explicit adversarial examples in training (Section 7); dedicated adversarial test set |
| **Self-correction ambiguity** — "actually" used as emphasis, not correction | Model deletes content after "actually" when speaker didn't correct | Medium | Generate examples where "actually" is NOT a correction marker ("I actually agree with you" → keep); balance with true correction examples |
