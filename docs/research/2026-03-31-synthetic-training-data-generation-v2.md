# Synthetic Training Data Generation for Transcript Cleanup Fine-Tuning (v2)

- **Version:** 2.0
- **Date:** 2026-03-31
- **Status:** Draft
- **Supersedes:** [2026-03-30-synthetic-training-data-generation.md](2026-03-30-synthetic-training-data-generation.md)

## Table of Contents

1. [Summary](#1-summary)
2. [Model Selection: LFM2.5-350M](#2-model-selection-lfm25-350m)
3. [Data Generation Philosophy](#3-data-generation-philosophy)
4. [The Three-Layer Generation Strategy](#4-the-three-layer-generation-strategy)
5. [Layer 1: Programmatic Corruption Engine](#5-layer-1-programmatic-corruption-engine)
6. [Layer 2: LLM-Generated Context-Dependent Patterns](#6-layer-2-llm-generated-context-dependent-patterns)
7. [Layer 3: TTS→ASR Round-Trip Pipeline](#7-layer-3-ttsasr-round-trip-pipeline)
8. [Source Text Acquisition](#8-source-text-acquisition)
9. [Domain and Persona Coverage](#9-domain-and-persona-coverage)
10. [Quality Assurance Pipeline](#10-quality-assurance-pipeline)
11. [Dataset Composition and Volume](#11-dataset-composition-and-volume)
12. [Output Format](#12-output-format)
13. [Validation Strategy](#13-validation-strategy)
14. [Cost and Timeline Estimate](#14-cost-and-timeline-estimate)
15. [Risks and Mitigations](#15-risks-and-mitigations)

---

## 1. Summary

This document describes a three-layer strategy for generating 25,000+ high-quality synthetic training pairs to fine-tune **LFM2.5-350M-Base** (LiquidAI) for speech-to-text transcript cleanup. Rather than relying on a single generation method, we combine three complementary approaches:

1. **Programmatic corruption** — deterministic disfluency injection into clean text (perfect ground truth, infinite variety, zero API cost)
2. **LLM-generated pairs** — Claude generates context-dependent patterns that can't be programmed (self-corrections, misheard words, natural speech flow)
3. **TTS→ASR round-trip** — real audio pipeline captures actual ASR error patterns from our engine

Each layer produces data with different strengths. Blending them creates a dataset that is diverse, precise, and grounded in real-world ASR behavior.

## 2. Model Selection: LFM2.5-350M

### Why LFM2.5-350M over SmolLM2-135M

| Property | LFM2.5-350M-Base | SmolLM2-135M |
|----------|------------------|--------------|
| Parameters | 350M | 135M |
| Architecture | Hybrid (10 conv + 6 GQA attention) | Transformer-only |
| Context window | 32,768 tokens | 2,048 tokens |
| Training data | 28T tokens | 11T tokens |
| Inference (CPU) | 313 tok/s | ~200 tok/s (est.) |
| Memory | <1GB | <500MB |
| MLX support | Yes (8-bit) | Yes |
| Fine-tuning rank | **#1 most tunable** (distil labs benchmark) | Not ranked |
| IFEval (pre-tuned) | 76.96 | ~40 (est.) |
| License | lfm1.0 | Apache 2.0 |

**Key deciding factors:**

1. **#1 most tunable model** — independent benchmarks (distil labs, March 2026) show LFM2 family absorbs fine-tuning signal more effectively than any other model family, including Qwen, Llama, and SmolLM2. Average tunability rank: 2.11/15.
2. **32K context window** — handles our longest possible transcription (12 min × 200 wpm = 2,400 words ≈ 2,400 tokens) with massive headroom. SmolLM2's 2K context would require chunking.
3. **Hybrid architecture** — the convolution blocks handle local patterns (filler detection, word repetition) natively, while attention blocks handle global context (self-correction across a sentence). This is architecturally well-suited to our task.
4. **313 tok/s on CPU** — fast enough for real-time cleanup even without GPU/ANE.

### Base vs Instruct

Use **LFM2.5-350M-Base**. The instruct variant has post-training alignment that teaches chat behavior we don't want. We're training a pure text-to-text function, not a conversational assistant. The base model's cleaner weight space absorbs our task-specific training signal without fighting pre-existing chat behaviors.

## 3. Data Generation Philosophy

### The Fundamental Insight

Most synthetic data guides describe a single approach: "ask a large LLM to generate (input, output) pairs." This has a critical flaw — **the teacher model's biases become the student's blind spots**. Claude generates speech patterns that sound like Claude imitating speech, not like real human speech.

Our approach inverts this. Instead of generating disfluent text, we:

1. **Start with real, diverse clean text** from public sources
2. **Corrupt it with mathematically precise disfluency injection** (programmatic)
3. **Supplement with LLM-generated patterns** only where programmatic methods can't reach (context-dependent self-corrections, misheard domain terms)
4. **Ground with real ASR artifacts** by running text through our actual TTS→ASR pipeline

This produces data where:
- The **ground truth is provably correct** (the clean text existed before corruption)
- The **disfluency patterns are statistically realistic** (based on linguistic research)
- The **ASR artifacts match our actual engine** (not simulated)
- The **content is genuinely diverse** (sourced from real text, not LLM-imagined)

### Prior Art

This approach is grounded in established NLP research:
- **LARD** (Passali et al., LREC 2022): Large-scale artificial disfluency generation via programmatic injection into clean text. Demonstrated effectiveness for training disfluency detection models.
- **Yang et al. (EMNLP 2020)**: "Planning and Generating Natural and Diverse Disfluent Texts as Augmentation" — showed that algorithmically planned disfluency insertion outperforms naive random insertion.
- **Ghosh et al. (ACL 2025)**: "Failing Forward" — demonstrated that synthetic ASR error data generated via TTS→ASR round-trip improves generative error correction models.

## 4. The Three-Layer Generation Strategy

```
                        ┌───────────────────────────────────────┐
                        │         SOURCE TEXT POOL               │
                        │  (Wikipedia, news, StackOverflow,      │
                        │   medical corpus, legal texts, etc.)   │
                        └──────────┬──────────┬─────────────────┘
                                   │          │
                    ┌──────────────┘          └──────────────┐
                    ▼                                        ▼
    ┌───────────────────────────┐         ┌──────────────────────────┐
    │  LAYER 1: Programmatic    │         │  LAYER 3: TTS→ASR        │
    │  Corruption Engine        │         │  Round-Trip Pipeline      │
    │                           │         │                          │
    │  • Filler injection       │         │  • macOS say / Coqui TTS │
    │  • Crutch word insertion  │         │  • FluidAudio ASR        │
    │  • Stutter/repetition     │         │  • Real error patterns   │
    │  • False start generation │         │  • Punctuation loss      │
    │  • Grammar degradation    │         │  • Capitalization loss   │
    │  • Punctuation removal    │         │                          │
    │  • Dictation cmd reversal │         │  Ground truth = original │
    │                           │         │  text before TTS         │
    │  Ground truth = original  │         └──────────────────────────┘
    │  text before corruption   │                    │
    └───────────────────────────┘                    │
                    │                                │
                    ▼                                ▼
    ┌───────────────────────────┐    ┌──────────────────────────┐
    │  LAYER 2: LLM-Generated   │    │                          │
    │  Context-Dependent Pairs   │    │                          │
    │                           │    │                          │
    │  • Self-corrections       │    │                          │
    │  • Misheard domain terms  │    │                          │
    │  • Adversarial examples   │    │        BLEND &            │
    │  • Natural mixed patterns │◄───┤        DEDUPLICATE        │
    │  • Long-form dictation    │    │                          │
    │                           │    │                          │
    │  Claude as teacher        │    │                          │
    └───────────────────────────┘    └──────────┬───────────────┘
                    │                            │
                    ▼                            ▼
              ┌──────────────────────────────────────┐
              │       QUALITY ASSURANCE PIPELINE      │
              │  Rule-based → LLM Judge → Dedup       │
              └──────────────────────────────────────┘
                              │
                              ▼
              ┌──────────────────────────────────────┐
              │       FINAL TRAINING DATASET          │
              │       (~25,000 pairs, JSONL)          │
              └──────────────────────────────────────┘
```

### Volume by Layer

| Layer | Pairs | % of total | Cost | Key strength |
|-------|-------|-----------|------|-------------|
| Layer 1: Programmatic | 14,000 | 56% | ~$0 (compute only) | Perfect ground truth, infinite variety |
| Layer 2: LLM-Generated | 8,000 | 32% | ~$60 (Claude API) | Context-dependent patterns |
| Layer 3: TTS→ASR | 3,000 | 12% | ~$0 (local compute) | Real ASR artifacts |
| **Total** | **25,000** | **100%** | **~$60** | |

## 5. Layer 1: Programmatic Corruption Engine

### Design Principles

The corruption engine takes clean text and applies a sequence of transformations, each with configurable probability and intensity. The original clean text is the ground truth — no ambiguity about what the "correct" output should be.

### 5.1 Corruption Operations

Each operation has parameters controlling frequency, position, and intensity.

#### Filler Injection

```python
FILLERS = ["uh", "um", "uhm", "er", "ah", "hmm"]
FILLER_WEIGHTS = [0.35, 0.30, 0.10, 0.10, 0.10, 0.05]  # frequency distribution

def inject_fillers(words, density=0.15):
    """Insert fillers at random word boundaries.
    
    density: probability of inserting a filler before each word.
    Higher density at clause boundaries and before complex words.
    """
```

**Placement heuristics (based on Shriberg, 1996):**
- 2x more likely at sentence/clause boundaries
- 1.5x more likely before words >3 syllables (cognitive load)
- Clustered: if a filler is inserted, 30% chance of a second filler within 3 words
- Density parameter: easy=0.05, medium=0.12, hard=0.25

#### Crutch Word Insertion

```python
CRUTCH_PATTERNS = {
    "sentence_start": ["so", "okay so", "alright so", "yeah so"],
    "hedging": ["basically", "essentially", "you know"],
    "reformulation": ["I mean", "like"],
    "agreement_seeking": ["right", "you know what I mean"],
    "topic_shift": ["anyway", "so anyway"],
    "intensifier": ["honestly", "literally"],
}

def inject_crutch_words(sentences, density=0.20):
    """Insert crutch words at sentence boundaries and mid-clause.
    
    Uses separate distributions for sentence-initial vs mid-sentence.
    """
```

**Placement:** 60% at sentence starts ("So basically..."), 40% mid-sentence ("...the thing is basically...").

#### Stutter / Word Repetition

```python
def inject_stutters(words, probability=0.08):
    """Repeat 1-3 words to simulate stuttering/false starts.
    
    Targets: articles (the, a, an), pronouns (I, we, it),
    sentence-initial words, and function words.
    """
```

**Patterns:**
- Single word: "the the server" (70% of stutters)
- Two words: "I think I think we should" (20%)
- Three words: "we need to we need to fix this" (10%)

#### False Start Generation

```python
def inject_false_starts(sentences, probability=0.10):
    """Generate abandoned sentence beginnings.
    
    Types:
    1. Synonym restart: replace first 2-4 words with synonyms, then continue
       "The function the method takes two parameters"
    2. Reframe restart: begin with alternative phrasing, abandon, restart
       "We need to we should probably add validation"
    3. Abandoned thought: start a clause, abandon, begin new thought
       "I was going to let's just merge it"
    """
```

#### Grammar Degradation

```python
GRAMMAR_RULES = {
    "gonna": (r"\b(going to)\b", "gonna", 0.7),
    "wanna": (r"\b(want to)\b", "wanna", 0.5),
    "gotta": (r"\b(got to|have to)\b", "gotta", 0.4),
    "its_confusion": (r"\b(it's)\b", "its", 0.3),
    "shouldve": (r"\b(should have)\b", "should of", 0.2),
    "drop_article": (r"\b(the|a|an) ", "", 0.1),
    "runon": (r"\. ([A-Z])", lambda m: " " + m.group(1).lower(), 0.3),
}

def degrade_grammar(text, intensity=0.3):
    """Apply spoken grammar patterns with configurable probability."""
```

#### Punctuation Stripping

```python
def strip_punctuation(text, completeness=0.9):
    """Remove punctuation to simulate raw ASR output.
    
    completeness: fraction of punctuation to remove (0.9 = remove 90%).
    Always lowercases. Converts newlines to spaces.
    """
```

This is applied to ALL Layer 1 outputs since ASR produces unpunctuated lowercase text.

#### Number Formatting Variation

```python
NUMBER_FORMATS = {
    "digit_to_word": True,    # "23" → "twenty three"
    "version_spoken": True,   # "2.0" → "two point oh"
    "ordinal_spoken": True,   # "3rd" → "third"
}

def vary_number_formatting(text, probability=0.3):
    """Convert some digits to spoken form, simulating ASR output.
    
    ASR often produces spoken numbers: "twenty three" not "23".
    The cleaned version should preserve the original format.
    """
```

**Note:** The cleaned version keeps numbers in their original written form. The model learns that "twenty three" in ASR output may map to "23" or "twenty-three" depending on the original context.

#### Dictation Command Reversal

```python
DICTATION_MAP = {
    ".": ["period", "full stop"],
    ",": ["comma"],
    "/": ["slash", "forward slash"],
    "?": ["question mark"],
    "!": ["exclamation point", "exclamation mark"],
    ":": ["colon"],
    ";": ["semicolon"],
    "-": ["dash", "hyphen"],
}

def reverse_dictation_commands(text, probability=0.15):
    """Replace punctuation with spoken equivalents.
    
    Only replaces a fraction of punctuation — simulates a user
    who sometimes says 'period' and sometimes just pauses.
    """
```

### 5.2 Corruption Pipeline

Operations are applied in a specific order to avoid conflicts:

```
clean_text
  → dictation_command_reversal (before punctuation is stripped)
  → number_formatting_variation (before punctuation is stripped)
  → grammar_degradation
  → false_start_injection
  → stutter_injection
  → crutch_word_insertion
  → filler_injection
  → punctuation_stripping + lowercasing
= raw_text
```

### 5.3 Difficulty Scaling

| Parameter | Easy | Medium | Hard |
|-----------|------|--------|------|
| Filler density | 0.05 | 0.12 | 0.25 |
| Crutch word density | 0.08 | 0.18 | 0.30 |
| Stutter probability | 0.03 | 0.08 | 0.15 |
| False start probability | 0.00 | 0.08 | 0.15 |
| Grammar degradation | 0.10 | 0.25 | 0.50 |
| Dictation command reversal | 0.05 | 0.15 | 0.30 |
| Corruption types applied | 1–2 | 2–4 | 4–6 |

### 5.4 Why This Layer Is Critical

- **Perfect ground truth.** The clean text exists BEFORE corruption. There is zero ambiguity about what the model should output.
- **No paraphrasing.** The clean version is verbatim the original — the model can never learn to rephrase, only to remove noise.
- **Infinite variety.** Random seeds produce unlimited unique examples from the same source text.
- **Zero cost.** No API calls. Runs locally in seconds.
- **Statistically grounded.** Disfluency placement follows empirical speech research (Shriberg, 1996; LARD, 2022).

### 5.5 Limitations

- **Self-corrections can't be programmatically generated** with natural semantics. "Send to dev, wait, actually send to staging" requires understanding that dev and staging are alternatives. This is why we need Layer 2.
- **Misheard words require domain knowledge.** Generating "oh auth" from "OAuth" requires knowing the phonetics. Layer 2 handles this.
- **Speech rhythm is missing.** Programmatic fillers are randomly placed; real fillers cluster around cognitive load points. Partially addressed by the placement heuristics.

## 6. Layer 2: LLM-Generated Context-Dependent Patterns

### What This Layer Produces

Patterns that require semantic understanding and can't be programmatically generated:

| Pattern | Why LLM is needed | Target volume |
|---------|-------------------|---------------|
| Self-corrections | Requires semantically valid alternatives | 3,000 |
| Misheard domain terms | Requires phonetic + domain knowledge | 1,500 |
| Natural mixed disfluencies | Realistic combination and flow | 1,500 |
| Adversarial examples | Requires understanding when NOT to clean | 1,000 |
| Long-form dictation | Complex multi-paragraph passages | 1,000 |

### Generation Prompt: Self-Corrections

This is our weakest category. The prompt must produce diverse correction patterns:

```
Generate 10 transcript pairs showing self-corrections in a {domain} context.
Speaker: {persona}

For each pair, the speaker starts saying one thing, then corrects themselves
using a marker word. The cleaned version should contain ONLY the corrected
version.

Correction markers to vary: "wait", "actually", "no", "scratch that",
"sorry", "I mean", "or rather", "hold on"

Requirements for RAW:
- No punctuation, all lowercase (simulating ASR output)
- The correction must be semantically meaningful (not just word substitution)
- Include the reasoning when natural ("wait that's too many, make it five")
- Vary whether the correction replaces a value, a target, a method, or a
  whole approach

Requirements for CLEAN:
- Contains ONLY the final/corrected intent
- Proper punctuation and capitalization
- No paraphrasing — words from the corrected portion of raw

Vary the patterns:
- Simple swap: "use X no use Y" → "Use Y."
- Value correction: "set it to 100 wait that's too low 500" → "Set it to 500."
- Target change: "send to A actually send to B" → "Send to B."
- Full rethink: "let's build X scratch that let's just use Y instead" → "Let's just use Y instead."
- Double correction: "X no Y actually Z" → "Z."
- Correction with reasoning: "X wait that won't scale Y" → "Y."

Output as JSON array with fields: raw, clean, correction_type, word_count
```

### Generation Prompt: Misheard Domain Terms

```
Generate 15 transcript pairs where an ASR system has misheard domain-specific
terminology in {domain}.

The ASR system produces phonetically plausible but semantically wrong text.
The surrounding context should make the correct term unambiguous.

For each pair, provide:
- raw: the ASR output (lowercase, no punctuation)
- clean: the corrected version
- misheard_term: what the ASR got wrong
- correct_term: what it should be

Requirements:
- The misheard version must be phonetically similar to the correct term
- Include both compound word splits ("web socket" → "WebSocket") and
  phonetic substitutions ("Kuber Netties" → "Kubernetes")
- The surrounding sentence should provide enough context that the correct
  term is unambiguous
- Include at least 3 examples where the "misheard" version is actually
  a real word ("patients" vs "patience", "bass" vs "base")

Domain-specific examples to inspire (but generate NEW ones):
Tech: "oh auth" → "OAuth", "post gress" → "Postgres"
Medical: "anna phylaxis" → "anaphylaxis"
Legal: "habeus" → "habeas"
Finance: "fiat" (correctly heard) vs "fight" (misheard)
```

### Generation Prompt: Adversarial Examples

```
Generate 15 TRICKY transcript pairs that test whether a cleanup model
correctly identifies what is and isn't a disfluency.

Categories of adversarial examples:

1. Filler words used meaningfully (4 examples):
   - "I like this approach" (keep "like")
   - "She is literally the CEO" (keep "literally" — it's factual)
   - "I mean the arithmetic mean" (keep "I mean")
   - "Right turn at the corner" (keep "right")

2. Correction markers used non-correctively (4 examples):
   - "I actually agree with you" (keep "actually" — not a correction)
   - "No, we shouldn't do that" (keep "no" — it's a negation, not correction)
   - "Wait for the build to finish" (keep "wait" — it's a command)

3. Dictation words as nouns (4 examples):
   - "The Victorian period was transformative" (keep "period")
   - "Use a forward slash in the regex" (keep "slash" — describing it)
   - "Add a comma operator in the expression" (keep "comma")

4. Intentional repetition for emphasis (3 examples):
   - "This is really really important" (keep the repetition)
   - "I need this done now now now" (keep — urgency emphasis)
   - "Never ever do that in production" (keep "ever")

For each pair: raw (lowercase, no punctuation) and clean (identical
content with only punctuation/capitalization added).
```

### Anchor Example Rotation

To prevent the teacher model from pattern-locking, rotate the 2–3 anchor examples in each batch from a pool of 30 pre-written diverse examples. Change anchors every 50 batches.

## 7. Layer 3: TTS→ASR Round-Trip Pipeline

### Concept

This layer captures **real artifacts from our actual ASR engine**. No simulation — the errors are exactly what users see.

```
clean_text → TTS engine → audio file → FluidAudio ASR → raw_text
                                                           ↓
                                               (clean_text, raw_text) pair
```

### Pipeline

1. **Source:** Take 3,000 clean sentences from the source text pool (Section 8)
2. **TTS:** Convert to audio using macOS `say` command (free, fast, multiple voices) or Coqui TTS for more variety
3. **ASR:** Run through FluidAudio (our production ASR engine) to get the raw transcript
4. **Pair:** The original clean text is the ground truth; the ASR output is the raw input

### What This Captures

| Artifact | Example |
|----------|---------|
| Compound word splits | "GitHub" → "git hub" |
| Capitalization loss | "OAuth" → "oauth" or "oh auth" |
| Punctuation loss | All punctuation stripped |
| Number formatting | "2.0" → "two point oh" |
| Homophones | "their" ↔ "there" ↔ "they're" |
| Jargon mishearing | Engine-specific patterns we can't predict |

### Limitations

- **No disfluencies.** TTS produces fluent speech — no "uh", "um", or self-corrections. This is why Layer 3 is only 12% of the dataset.
- **TTS artifacts.** Robotic pronunciation may cause ASR errors that real speech wouldn't.
- **Slow.** Each sentence requires TTS synthesis + ASR inference (~2–5 seconds). 3,000 sentences ≈ 2–4 hours.

### Voice Diversity

Use multiple TTS voices to maximize ASR error variety:
- macOS `say` voices: Alex, Samantha, Daniel, Karen, Moira, Tessa (~6 English voices with distinct pronunciation)
- Each voice produces different prosody → different ASR error patterns
- Distribute 3,000 sentences evenly across voices (~500 per voice)

### Enhancement: Disfluency-Injected TTS

For a subset (500 samples), apply Layer 1 corruption to the clean text **before** TTS. The TTS will speak the fillers aloud, and the ASR will transcribe them naturally (or drop them). This creates samples with both disfluencies AND real ASR artifacts:

```
clean_text → Layer 1 corruption → corrupted_text → TTS → audio → ASR → raw_text
                                                                          ↓
                                                           (clean_text, raw_text) pair
```

## 8. Source Text Acquisition

### The Key Insight

The quality and diversity of the **source clean text** is as important as the corruption method. If all source text is Wikipedia-style encyclopedic prose, the model learns to clean only formal text. We need diverse registers, domains, and sentence structures.

### Sources (All Public, Freely Licensed)

| Source | Domain coverage | Register | Estimated sentences |
|--------|----------------|----------|-------------------|
| **Wikipedia** (random articles) | All domains | Encyclopedic/formal | 5,000 |
| **StackOverflow** (questions + accepted answers) | Software engineering | Technical/conversational | 3,000 |
| **Reddit** (top comments, curated subreddits) | General, tech, finance | Casual/conversational | 3,000 |
| **PubMed abstracts** | Medical/health | Scientific/formal | 1,500 |
| **SEC filings** (EDGAR, plain-text sections) | Finance/legal | Formal/regulatory | 1,000 |
| **arXiv abstracts** | Academic/research | Scientific | 1,000 |
| **Project Gutenberg** (modern section) | Creative/literary | Narrative | 500 |
| **News articles** (CC-licensed) | General/business | Journalistic | 2,000 |
| **Meeting transcripts** (AMI corpus, public) | Business meetings | Spoken/natural | 1,000 |
| **Custom-written** (by us, for SottoASR use cases) | Software eng + dictation | Natural dictation | 500 |

**Total:** ~18,500 unique source sentences, each used 1–2 times across layers.

### Validation Grounding: Real Disfluency Corpora

While we don't use real corpora for training (licensing, domain mismatch), we can use them to validate that our synthetic disfluencies are statistically realistic:

- **Switchboard NXT Corpus** — gold-standard disfluency annotations from telephone conversations. Use to verify our filler placement distributions match real speech.
- **AMI Meeting Corpus** — annotated meeting transcripts with disfluencies marked. Compare our crutch word and false start rates.
- **Santa Barbara Corpus of Spoken American English** — diverse spoken language with detailed disfluency coding.

These serve as a sanity check, not training data.

### Source Text Processing

Before feeding into any layer:
1. **Sentence segmentation** — split paragraphs into individual sentences or 2–3 sentence chunks (for medium/long examples)
2. **Length filtering** — keep sentences with 5–60 words (single) or 20–200 words (chunks)
3. **Quality filtering** — remove fragments, tables, code blocks, URLs, non-English text
4. **Deduplication** — remove near-duplicates (cosine similarity > 0.90)
5. **Domain labeling** — auto-classify each sentence by domain using keyword matching

## 9. Domain and Persona Coverage

### Domain Distribution

| Domain | % of dataset | Source weighting |
|--------|-------------|-----------------|
| **Software engineering** | 25% | StackOverflow, GitHub discussions |
| **General business** | 20% | News, Reddit (business subs), meeting transcripts |
| **Casual/conversational** | 15% | Reddit (casual subs), custom-written |
| **Technical (non-software)** | 10% | Wikipedia (tech articles), arXiv |
| **Medical/health** | 8% | PubMed, Wikipedia (medical) |
| **Legal/compliance** | 7% | SEC filings, Wikipedia (law) |
| **Academic/research** | 5% | arXiv, Wikipedia (science) |
| **Creative/content** | 5% | Project Gutenberg, Reddit (writing subs) |
| **Finance** | 5% | SEC filings, news (finance) |

### Speaker Personas (Layer 1 & 2)

Personas control the corruption parameters, not the content. A "junior developer" persona means higher filler density and more hedging crutch words; a "senior engineer" means fewer fillers but more technical jargon.

| Persona | Filler density | Crutch density | Self-correction rate | Grammar degradation |
|---------|---------------|----------------|---------------------|-------------------|
| Senior engineer | Low (0.05) | Low (0.05) | Low (0.03) | Low (0.05) |
| Junior developer | High (0.20) | High (0.25) | Medium (0.10) | Medium (0.20) |
| Manager | Low (0.08) | Medium (0.15) | Low (0.05) | Low (0.08) |
| Non-native speaker | Medium (0.12) | Low (0.08) | Medium (0.10) | High (0.35) |
| Fast talker | Medium (0.10) | High (0.20) | High (0.18) | Medium (0.15) |
| Deliberate speaker | Very low (0.03) | Low (0.05) | Very low (0.02) | Very low (0.03) |
| Domain expert | Low (0.06) | Medium (0.12) | Low (0.04) | Low (0.05) |
| Casual dictator | Medium (0.10) | Low (0.08) | Low (0.05) | Medium (0.20) |

## 10. Quality Assurance Pipeline

### Stage 1: Rule-Based Validation (All Layers)

| Check | Rule | Action |
|-------|------|--------|
| Non-empty | Both fields ≥ 3 words | Reject |
| Length ratio | 0.2 ≤ len(clean)/len(raw) ≤ 1.05 | Reject |
| No fillers in clean | Regex: no uh/um/uhm/er/ah as standalone words | Reject |
| No crutch words at sentence start in clean | Regex: no ^(so\|basically\|okay\|anyway) | Flag |
| Proper capitalization | First letter uppercase after sentence boundary | Flag |
| Ends with punctuation | clean ends with ./?/! | Flag |
| Content preservation | Jaccard word overlap ≥ 0.5 (excluding stopwords + fillers) | Reject |

### Stage 2: Automated Semantic Check (Layer 2 only)

For LLM-generated pairs, run an automated check with a fast model:

```
Given this (raw, clean) pair, answer YES or NO:
1. Does the clean version preserve all meaningful content from raw?
2. Does the clean version avoid paraphrasing or rewording?
3. If there's a self-correction in raw, does clean contain ONLY the corrected version?
If any answer is NO, output REJECT with the reason.
```

### Stage 3: LLM Judge (10% Random Sample)

Score 2,500 random samples on:
- **Faithfulness (1–5):** Meaning preserved?
- **Completeness (1–5):** All disfluencies handled?
- **Naturalness (1–5):** Does the raw text sound like real speech?

Reject samples scoring < 3 on any dimension. If batch rejection rate exceeds 20%, investigate the generation step that produced them.

### Stage 4: Deduplication

1. Compute sentence embeddings for all `raw` fields using `sentence-transformers/all-MiniLM-L6-v2`
2. Remove pairs where raw cosine similarity > 0.90
3. Verify zero overlap with the 135-sample benchmark set (cosine < 0.85)

### Stage 5: Distribution Audit

Verify the final dataset matches target proportions within tolerance:
- Category: ±5%
- Domain: ±3%
- Difficulty: ±5%
- Length bucket: ±5%

Resample or generate additional batches to correct imbalances.

## 11. Dataset Composition and Volume

### By Category

| Category | Layer 1 | Layer 2 | Layer 3 | Total | % |
|----------|---------|---------|---------|-------|---|
| filler_removal | 2,500 | 0 | 500 | 3,000 | 12% |
| crutch_words | 2,000 | 0 | 0 | 2,000 | 8% |
| self_correction | 0 | 3,000 | 0 | 3,000 | 12% |
| false_start | 1,500 | 500 | 0 | 2,000 | 8% |
| grammar | 1,500 | 0 | 500 | 2,000 | 8% |
| misheard_words | 0 | 1,500 | 500 | 2,000 | 8% |
| dictation_commands | 2,000 | 0 | 0 | 2,000 | 8% |
| list_formatting | 1,000 | 500 | 0 | 1,500 | 6% |
| preserve_wording | 2,000 | 1,000 | 1,000 | 4,000 | 16% |
| mixed (multi-type) | 1,500 | 1,500 | 500 | 3,500 | 14% |
| **Total** | **14,000** | **8,000** | **3,000** | **25,000** | **100%** |

**Note:** preserve_wording is 16% of the dataset — intentionally high. The model must learn that clean or near-clean input should pass through unchanged. This prevents the over-correction bias that plagued our prompted 2B model.

### By Difficulty

| Difficulty | Count | % |
|------------|-------|---|
| Easy (1 disfluency, 5–25 words) | 7,500 | 30% |
| Medium (2–3 disfluencies, 15–60 words) | 11,250 | 45% |
| Hard (4+ disfluencies, 40–250 words) | 6,250 | 25% |

### Splits

| Split | Count | Source |
|-------|-------|--------|
| Training | 22,000 | Stratified sample from all layers |
| Validation | 2,500 | Stratified sample, held out |
| Test | 500 | Held out, never seen during development |
| Benchmark | 135 | Existing hand-curated set (completely separate) |

## 12. Output Format

### Training Data (JSONL)

```jsonl
{"input": "the uh server is uh running low on memory", "output": "The server is running low on memory."}
{"input": "use python actually no lets use rust for this", "output": "Let's use Rust for this."}
{"input": "ship it", "output": "Ship it."}
```

**LFM2.5-350M-Base is decoder-only.** The training script concatenates input and output with separator tokens. The loss is computed only on the output portion:

```
<|startoftext|>INPUT: the uh server is uh running low on memory
OUTPUT: The server is running low on memory.<|endoftext|>
```

The exact separator format will be determined during the training setup phase, but the JSONL format decouples data generation from model-specific formatting.

### Metadata Sidecar (metadata.jsonl)

```jsonl
{"id": "L1_00001", "layer": 1, "category": "filler_removal", "domain": "software_engineering", "difficulty": "easy", "persona": "senior_engineer", "source": "stackoverflow", "word_count_raw": 12, "word_count_clean": 9, "disfluency_tags": ["filler"], "corruption_params": {"filler_density": 0.05}}
```

## 13. Validation Strategy

### Pre-Training

1. **Parse every JSONL line** — must be valid JSON with `input` and `output`
2. **Run quality pipeline** (Section 10) — reject rate target: < 25%
3. **Distribution audit** — verify proportions match Section 11
4. **Manual review** — read 200 random samples across all layers
5. **Benchmark isolation** — verify zero cosine overlap with 135-sample benchmark

### Post-Training

1. **Benchmark regression** — run 135-sample benchmark, compare to prompted 2B baseline
2. **Per-category targets:**

| Category | Prompted 2B baseline | Fine-tuned LFM2.5 target |
|----------|---------------------|-------------------------|
| self_correction | 0.742 | **> 0.90** |
| crutch_words | 0.879 | **> 0.95** |
| filler_removal | 0.974 | **> 0.99** |
| dictation_commands | 0.992 | **> 0.99** |
| preserve_wording | 0.992 | **> 0.99** |
| list_formatting | 0.859 | **> 0.90** |
| grammar | 0.874 | **> 0.90** |
| misheard_words | 0.923 | **> 0.90** |
| **Overall ROUGE-L** | **0.891** | **> 0.95** |

3. **Adversarial test** — 50 adversarial examples where filler words are meaningful
4. **Domain generalization** — 50 held-out examples per domain
5. **Identity preservation** — 100 clean inputs, verify output ≈ input
6. **Length stress test** — 20 examples of 200+ words, verify no truncation or hallucination

## 14. Cost and Timeline Estimate

### Costs

| Item | Cost |
|------|------|
| Layer 1: Programmatic generation | $0 (local CPU) |
| Layer 2: Claude API (~8,000 samples) | ~$60 |
| Layer 3: TTS→ASR round-trip | $0 (local compute) |
| LLM judge (10% sample) | ~$8 |
| Sentence embeddings for dedup | $0 (local, MiniLM) |
| Fine-tuning (LoRA, 3 epochs on A10) | ~$5–10 |
| **Total** | **~$75–80** |

### Timeline

| Phase | Duration | Output |
|-------|----------|--------|
| Source text acquisition + processing | 1 day | 18,500 clean sentences |
| Layer 1: Build corruption engine + generate | 2 days | 14,000 pairs |
| Layer 2: LLM generation + validation | 1 day | 8,000 pairs |
| Layer 3: TTS→ASR pipeline | 1 day | 3,000 pairs |
| Quality assurance + dedup + balancing | 1 day | 25,000 final pairs |
| Fine-tuning + evaluation | 1 day | Trained model |
| **Total** | **~7 days** | |

## 15. Risks and Mitigations

| Risk | Impact | Likelihood | Mitigation |
|------|--------|------------|------------|
| **Programmatic corruption sounds unnatural** | Model learns artificial patterns | Medium | Placement heuristics based on linguistic research; persona-based parameter variation; Layer 2 provides natural counterbalance |
| **Teacher model bias in Layer 2** | Student inherits Claude's speech simulation style | Medium | Layer 2 is only 32% of data; high temperature (0.9); persona and domain rotation; anchor example rotation every 50 batches |
| **TTS→ASR artifacts differ from real speech** | Layer 3 data doesn't match real usage | Low | Layer 3 is only 12% and focused on ASR-specific patterns; supplement with real user data when available |
| **Over-correction bias** | Model changes clean text unnecessarily | High | 16% preserve_wording examples; dedicated adversarial test set; identity preservation validation |
| **Self-correction ambiguity** | "actually" used as emphasis gets treated as correction | Medium | Adversarial examples in training; balance of correction vs non-correction uses of marker words |
| **Domain blind spots** | Underrepresented domains fail | Low | 9 domains with stratified coverage; per-domain held-out test |
| **LFM2.5 MLX fine-tuning issues** | Model doesn't fine-tune well via MLX | Medium | Fine-tune via PyTorch/Unsloth on GPU; convert to MLX after training; LiquidAI provides Unsloth fine-tuning notebooks |
| **Distributional mismatch with real ASR** | Synthetic data doesn't match FluidAudio output patterns | Medium | Layer 3 uses actual FluidAudio engine; plan to add real user transcriptions when available |
| **Context window utilization** | Model trained on mostly short examples fails on long input | Low | 10% of dataset is 150–250 words; 25% is 50–150 words; length stress test in validation |
