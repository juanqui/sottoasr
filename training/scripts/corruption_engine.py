#!/usr/bin/env python3
"""Layer 1: Programmatic Corruption Engine for transcript cleanup training data.

Takes clean text and applies disfluency injection to produce (raw, clean) pairs
with perfect ground truth. Based on Shriberg (1996) disfluency placement heuristics
and the LARD methodology (Passali et al., LREC 2022).

Usage:
    python corruption_engine.py --source source_text/ --output layer1_programmatic/ --count 14000
    python corruption_engine.py --source source_text/ --output layer1_programmatic/ --count 100 --preview
"""

import argparse
import json
import random
import re
import sys
from pathlib import Path
from dataclasses import dataclass, asdict, field


# ---------------------------------------------------------------------------
# Disfluency Inventories
# ---------------------------------------------------------------------------

FILLERS = ["uh", "um", "uhm", "er", "ah", "hmm"]
FILLER_WEIGHTS = [0.35, 0.30, 0.10, 0.10, 0.10, 0.05]

CRUTCH_SENTENCE_START = [
    "so", "okay so", "alright so", "yeah so", "okay", "well",
    "so yeah", "right so",
]
CRUTCH_HEDGING = ["basically", "essentially", "you know"]
CRUTCH_REFORMULATION = ["I mean", "like"]
CRUTCH_AGREEMENT = ["right", "you know what I mean"]
CRUTCH_TOPIC_SHIFT = ["anyway", "so anyway", "anyhow"]
CRUTCH_INTENSIFIER = ["honestly", "literally"]

GRAMMAR_CONTRACTIONS = [
    (r"\b[Gg]oing to\b", "gonna"),
    (r"\b[Ww]ant to\b", "wanna"),
    (r"\b[Gg]ot to\b", "gotta"),
    (r"\b[Hh]ave to\b", "gotta"),
]

GRAMMAR_ERRORS = [
    (r"\b[Ii]t's\b", "its"),
    (r"\b[Ss]hould have\b", "should of"),
    (r"\b[Cc]ould have\b", "could of"),
    (r"\b[Ww]ould have\b", "would of"),
]

DICTATION_MAP = {
    ".": ["period", "full stop"],
    ",": ["comma"],
    "/": ["slash", "forward slash"],
    "?": ["question mark"],
    "!": ["exclamation point", "exclamation mark"],
    ":": ["colon"],
    ";": ["semicolon"],
}

NUMBER_WORDS = {
    "0": "zero", "1": "one", "2": "two", "3": "three", "4": "four",
    "5": "five", "6": "six", "7": "seven", "8": "eight", "9": "nine",
    "10": "ten", "11": "eleven", "12": "twelve", "13": "thirteen",
    "14": "fourteen", "15": "fifteen", "16": "sixteen", "17": "seventeen",
    "18": "eighteen", "19": "nineteen", "20": "twenty", "30": "thirty",
    "40": "forty", "50": "fifty", "60": "sixty", "70": "seventy",
    "80": "eighty", "90": "ninety", "100": "a hundred", "1000": "a thousand",
}


# ---------------------------------------------------------------------------
# Persona Configurations
# ---------------------------------------------------------------------------

@dataclass
class PersonaConfig:
    name: str
    filler_density: float
    crutch_density: float
    stutter_prob: float
    false_start_prob: float
    grammar_degradation: float
    dictation_reversal: float

PERSONAS = [
    PersonaConfig("senior_engineer", 0.05, 0.05, 0.03, 0.02, 0.05, 0.08),
    PersonaConfig("junior_developer", 0.20, 0.25, 0.10, 0.08, 0.20, 0.05),
    PersonaConfig("manager", 0.08, 0.15, 0.04, 0.03, 0.08, 0.10),
    PersonaConfig("non_native_speaker", 0.12, 0.08, 0.06, 0.05, 0.35, 0.03),
    PersonaConfig("fast_talker", 0.10, 0.20, 0.12, 0.15, 0.15, 0.05),
    PersonaConfig("deliberate_speaker", 0.03, 0.05, 0.02, 0.01, 0.03, 0.12),
    PersonaConfig("domain_expert", 0.06, 0.12, 0.03, 0.02, 0.05, 0.08),
    PersonaConfig("casual_dictator", 0.10, 0.08, 0.05, 0.03, 0.20, 0.25),
]


# ---------------------------------------------------------------------------
# Corruption Operations
# ---------------------------------------------------------------------------

def syllable_count(word: str) -> int:
    """Rough syllable count for English words."""
    word = word.lower().strip(".,!?;:'\"")
    if len(word) <= 3:
        return 1
    vowels = "aeiouy"
    count = 0
    prev_vowel = False
    for ch in word:
        is_vowel = ch in vowels
        if is_vowel and not prev_vowel:
            count += 1
        prev_vowel = is_vowel
    if word.endswith("e") and count > 1:
        count -= 1
    return max(1, count)


def inject_fillers(words: list[str], density: float, rng: random.Random) -> list[str]:
    """Insert fillers at word boundaries with placement heuristics."""
    if density <= 0 or not words:
        return words

    result = []
    clause_boundary_words = {"and", "but", "or", "so", "because", "when", "if", "then", "that", "which"}

    for i, word in enumerate(words):
        # Base probability
        prob = density

        # 2x at clause boundaries
        if word.lower().rstrip(".,!?") in clause_boundary_words:
            prob *= 2.0

        # 1.5x before complex words (>2 syllables)
        if syllable_count(word) > 2:
            prob *= 1.5

        # 2x at sentence start
        if i == 0:
            prob *= 2.0

        # Clustering: higher if previous word was a filler
        if result and result[-1] in FILLERS:
            prob *= 0.3  # 30% chance of adjacent filler

        if rng.random() < min(prob, 0.5):  # cap at 50%
            filler = rng.choices(FILLERS, weights=FILLER_WEIGHTS, k=1)[0]
            result.append(filler)

        result.append(word)

    return result


def inject_crutch_words(text: str, density: float, rng: random.Random) -> str:
    """Insert crutch words at sentence boundaries and mid-clause."""
    if density <= 0:
        return text

    sentences = re.split(r'(?<=[.!?])\s+', text)
    result = []

    for sent in sentences:
        # 60% chance: add crutch at sentence start
        if rng.random() < density * 0.6:
            crutch = rng.choice(CRUTCH_SENTENCE_START)
            sent = f"{crutch} {sent[0].lower()}{sent[1:]}" if sent else sent

        # 40% chance: insert hedging/reformulation mid-sentence
        if rng.random() < density * 0.4:
            words = sent.split()
            if len(words) > 4:
                pos = rng.randint(2, len(words) - 2)
                crutch = rng.choice(CRUTCH_HEDGING + CRUTCH_REFORMULATION)
                words.insert(pos, crutch)
                sent = " ".join(words)

        result.append(sent)

    return " ".join(result)


def inject_stutters(words: list[str], probability: float, rng: random.Random) -> list[str]:
    """Repeat 1-3 words to simulate stuttering."""
    if probability <= 0 or len(words) < 3:
        return words

    result = []
    stutter_targets = {"the", "a", "an", "i", "we", "it", "he", "she", "they", "this", "that", "is", "was"}
    i = 0

    while i < len(words):
        word = words[i]
        # Higher probability for articles/pronouns and sentence-initial words
        p = probability
        if word.lower().rstrip(".,!?") in stutter_targets:
            p *= 2.0
        if i == 0:
            p *= 1.5

        if rng.random() < p:
            # Choose stutter length
            stutter_type = rng.choices([1, 2, 3], weights=[0.70, 0.20, 0.10], k=1)[0]
            repeat_count = min(stutter_type, len(words) - i)
            repeated = words[i:i + repeat_count]
            result.extend(repeated)  # stutter (repeated words)
            result.extend(repeated)  # then the actual words
            i += repeat_count
        else:
            result.append(word)
            i += 1

    return result


def inject_false_starts(text: str, probability: float, rng: random.Random) -> str:
    """Generate abandoned sentence beginnings."""
    if probability <= 0:
        return text

    sentences = re.split(r'(?<=[.!?])\s+', text)
    result = []

    for sent in sentences:
        if rng.random() < probability and len(sent.split()) > 5:
            words = sent.split()
            # Take first 2-4 words, then restart
            restart_len = rng.randint(2, min(4, len(words) - 2))
            false_start = " ".join(words[:restart_len])
            result.append(f"{false_start} {sent}")
        else:
            result.append(sent)

    return " ".join(result)


def degrade_grammar(text: str, intensity: float, rng: random.Random) -> str:
    """Apply spoken grammar patterns."""
    if intensity <= 0:
        return text

    # Contractions
    for pattern, replacement in GRAMMAR_CONTRACTIONS:
        if rng.random() < intensity:
            text = re.sub(pattern, replacement, text, count=1)

    # Grammar errors
    for pattern, replacement in GRAMMAR_ERRORS:
        if rng.random() < intensity * 0.5:
            text = re.sub(pattern, replacement, text, count=1)

    # Run-on sentences (join two sentences)
    if rng.random() < intensity * 0.3:
        text = re.sub(r'\. ([A-Z])', lambda m: " " + m.group(1).lower(), text, count=1)

    return text


def reverse_dictation_commands(text: str, probability: float, rng: random.Random) -> str:
    """Replace some punctuation with spoken equivalents."""
    if probability <= 0:
        return text

    for punct, spoken_forms in DICTATION_MAP.items():
        if punct in text and rng.random() < probability:
            spoken = rng.choice(spoken_forms)
            # Only replace one occurrence
            text = text.replace(punct, f" {spoken}", 1)

    return text


def strip_punctuation_and_lowercase(text: str, completeness: float, rng: random.Random) -> str:
    """Remove punctuation and lowercase to simulate ASR output."""
    result = text.lower()

    if completeness >= 1.0:
        result = re.sub(r'[.!?,;:\'"()\[\]{}\-—–/]', ' ', result)
    else:
        for ch in '.!?,;:\'"()-/':
            if rng.random() < completeness:
                result = result.replace(ch, ' ')

    # Collapse whitespace
    result = re.sub(r'\s+', ' ', result).strip()
    return result


# ---------------------------------------------------------------------------
# Main Corruption Pipeline
# ---------------------------------------------------------------------------

@dataclass
class CorruptionResult:
    id: str
    input: str  # raw (corrupted)
    output: str  # clean (original)
    category: str
    domain: str
    difficulty: str
    persona: str
    source: str
    word_count_raw: int
    word_count_clean: int
    disfluency_tags: list[str] = field(default_factory=list)


def corrupt(
    clean_text: str,
    persona: PersonaConfig,
    difficulty: str,
    domain: str,
    source: str,
    sample_id: str,
    rng: random.Random,
) -> CorruptionResult:
    """Apply the full corruption pipeline to a clean text sample."""

    # Difficulty multipliers
    multiplier = {"easy": 0.5, "medium": 1.0, "hard": 2.0}[difficulty]

    # Scale persona params by difficulty
    fd = min(persona.filler_density * multiplier, 0.4)
    cd = min(persona.crutch_density * multiplier, 0.4)
    sp = min(persona.stutter_prob * multiplier, 0.25)
    fsp = min(persona.false_start_prob * multiplier, 0.20)
    gd = min(persona.grammar_degradation * multiplier, 0.6)
    dr = min(persona.dictation_reversal * multiplier, 0.4)

    tags = []
    text = clean_text

    # 1. Dictation command reversal (before punctuation strip)
    if dr > 0 and rng.random() < 0.5:
        text = reverse_dictation_commands(text, dr, rng)
        if text != clean_text:
            tags.append("dictation_commands")

    # 2. Grammar degradation
    before = text
    text = degrade_grammar(text, gd, rng)
    if text != before:
        tags.append("grammar")

    # 3. False start injection
    before = text
    text = inject_false_starts(text, fsp, rng)
    if text != before:
        tags.append("false_start")

    # 4. Crutch word insertion
    before = text
    text = inject_crutch_words(text, cd, rng)
    if text != before:
        tags.append("crutch_words")

    # 5. Stutter + filler injection (word-level)
    words = text.split()
    before_words = list(words)
    words = inject_stutters(words, sp, rng)
    if words != before_words:
        tags.append("false_start")  # stutters are a form of false start
    words = inject_fillers(words, fd, rng)
    if any(w in FILLERS for w in words if w not in before_words):
        tags.append("filler_removal")
    text = " ".join(words)

    # 6. Punctuation stripping + lowercasing (always applied)
    raw = strip_punctuation_and_lowercase(text, 0.95, rng)

    # Deduplicate tags
    tags = list(dict.fromkeys(tags))
    if not tags:
        tags = ["preserve_wording"]

    # Determine primary category
    category = tags[0] if tags[0] != "preserve_wording" else "preserve_wording"

    return CorruptionResult(
        id=sample_id,
        input=raw,
        output=clean_text,
        category=category,
        domain=domain,
        difficulty=difficulty,
        persona=persona.name,
        source=source,
        word_count_raw=len(raw.split()),
        word_count_clean=len(clean_text.split()),
        disfluency_tags=tags,
    )


# ---------------------------------------------------------------------------
# Source Text Loading
# ---------------------------------------------------------------------------

def load_source_texts(source_dir: Path) -> list[dict]:
    """Load source text files. Each file is JSONL with {text, domain, source} fields."""
    texts = []
    for f in sorted(source_dir.glob("*.jsonl")):
        with open(f) as fh:
            for line in fh:
                line = line.strip()
                if line:
                    texts.append(json.loads(line))
    return texts


# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------

def main():
    parser = argparse.ArgumentParser(description="Layer 1: Programmatic Corruption Engine")
    parser.add_argument("--source", type=Path, required=True, help="Directory with source text JSONL files")
    parser.add_argument("--output", type=Path, required=True, help="Output directory for generated pairs")
    parser.add_argument("--count", type=int, default=14000, help="Number of pairs to generate")
    parser.add_argument("--seed", type=int, default=42, help="Random seed")
    parser.add_argument("--preview", action="store_true", help="Print first 10 samples instead of writing")
    args = parser.parse_args()

    rng = random.Random(args.seed)

    # Load source texts
    sources = load_source_texts(args.source)
    if not sources:
        print(f"No source texts found in {args.source}", file=sys.stderr)
        sys.exit(1)

    print(f"Loaded {len(sources)} source sentences")

    difficulties = ["easy", "medium", "medium", "medium", "hard"]  # weighted toward medium
    results = []

    for i in range(args.count):
        # Pick random source, persona, difficulty
        src = rng.choice(sources)
        persona = rng.choice(PERSONAS)
        difficulty = rng.choice(difficulties)

        result = corrupt(
            clean_text=src["text"],
            persona=persona,
            difficulty=difficulty,
            domain=src.get("domain", "general"),
            source=src.get("source", "unknown"),
            sample_id=f"L1_{i:05d}",
            rng=rng,
        )
        results.append(result)

    if args.preview:
        for r in results[:10]:
            print(f"\n--- {r.id} ({r.category}, {r.difficulty}, {r.persona}) ---")
            print(f"  CLEAN: {r.output}")
            print(f"  RAW:   {r.input}")
            print(f"  TAGS:  {r.disfluency_tags}")
        print(f"\n... {len(results)} total samples generated")
    else:
        args.output.mkdir(parents=True, exist_ok=True)
        # Write training data
        train_file = args.output / "layer1_train.jsonl"
        meta_file = args.output / "layer1_metadata.jsonl"
        with open(train_file, "w") as tf, open(meta_file, "w") as mf:
            for r in results:
                tf.write(json.dumps({"input": r.input, "output": r.output}) + "\n")
                mf.write(json.dumps(asdict(r)) + "\n")
        print(f"Written {len(results)} pairs to {train_file}")
        print(f"Metadata written to {meta_file}")

        # Stats
        from collections import Counter
        cats = Counter(r.category for r in results)
        diffs = Counter(r.difficulty for r in results)
        domains = Counter(r.domain for r in results)
        print(f"\nCategory distribution:")
        for k, v in sorted(cats.items(), key=lambda x: -x[1]):
            print(f"  {k:25s}: {v:5d} ({v/len(results)*100:.1f}%)")
        print(f"\nDifficulty distribution:")
        for k, v in sorted(diffs.items()):
            print(f"  {k:10s}: {v:5d} ({v/len(results)*100:.1f}%)")


if __name__ == "__main__":
    main()
