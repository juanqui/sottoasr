#!/usr/bin/env python3
"""
SottoASR Synthetic Training Data Generator
==========================================

Generates high-quality (raw_transcript, cleaned_transcript) training pairs
for fine-tuning a transcript cleanup model. Uses an OpenAI-compatible API
(vLLM endpoint) to generate diverse, context-dependent examples at scale.

Architecture:
  - Async worker pool with bounded concurrency (no task overshoot)
  - Persona × Domain × Category × Difficulty matrix for diversity
  - Multiple generation strategies (direct, corruption-based, adversarial)
  - Built-in validation, deduplication, and quality filtering
  - Checkpoint/resume support for long runs
  - Progress tracking with ETA

Usage:
  python generate_synthetic.py --target 100000 --concurrency 8
  python generate_synthetic.py --target 5000 --category self_correction --concurrency 4
  python generate_synthetic.py --resume  # resume from last checkpoint

Environment:
  VLLM_BASE_URL  - vLLM endpoint (default: http://192.168.1.128:8000/v1)
  VLLM_API_KEY   - API key if required (default: none)
  VLLM_MODEL     - Model name (default: auto-detect from endpoint)

Note: Random seed controls diversity matrix selection but async task ordering
is non-deterministic. Same seed + same endpoint will produce similar but not
identical datasets across runs when concurrency > 1.
"""

import argparse
import asyncio
import json
import hashlib
import logging
import os
import random
import re
import sys
import time
from collections import Counter
from dataclasses import dataclass, asdict
from pathlib import Path
from typing import Optional

try:
    import aiohttp
except ImportError:
    print("Error: aiohttp is required. Install with: pip install aiohttp", file=sys.stderr)
    sys.exit(1)

# ---------------------------------------------------------------------------
# Configuration
# ---------------------------------------------------------------------------

DEFAULT_BASE_URL = "http://192.168.1.128:8000/v1"
DEFAULT_CONCURRENCY = 8
DEFAULT_TARGET = 100_000
DEFAULT_OUTPUT_DIR = Path(__file__).parent.parent / "data" / "generated"

# Generation parameters — high temperature for diversity
TEMPERATURE = 0.9
TOP_P = 0.95
MAX_TOKENS = 2048  # Enough for 10 samples per batch without thinking tokens
BATCH_SIZE = 10  # samples requested per API call

# Validation thresholds
MIN_INPUT_WORDS = 2
MAX_INPUT_WORDS = 1500   # Support long transcripts (up to ~7 min at 200 wpm)
MIN_OUTPUT_WORDS = 1
MAX_OUTPUT_WORDS = 1500
MIN_LENGTH_RATIO = 0.15   # clean/raw (aggressive self-corrections can be very short)
MAX_LENGTH_RATIO = 1.15   # clean should never be much longer than raw

# ---------------------------------------------------------------------------
# Domains, Personas, Categories — the diversity matrix
# ---------------------------------------------------------------------------

DOMAINS = [
    {
        "name": "software_engineering",
        "weight": 0.25,
        "topics": [
            "deploying a web application", "debugging a production issue",
            "code review feedback", "setting up CI/CD pipeline",
            "database migration", "API design discussion",
            "microservices architecture", "container orchestration",
            "monitoring and alerting", "performance optimization",
            "security vulnerability remediation", "feature flag rollout",
            "incident postmortem", "sprint planning", "technical debt",
            "load testing results", "infrastructure as code",
            "dependency updates", "branch strategy", "release process",
        ],
        "jargon": [
            "Kubernetes", "Docker", "PostgreSQL", "Redis", "GraphQL",
            "REST API", "WebSocket", "CI/CD", "Terraform", "ArgoCD",
            "GitHub Actions", "Prometheus", "Grafana", "Elasticsearch",
            "Kafka", "RabbitMQ", "gRPC", "OAuth", "JWT", "Nginx",
            "Svelte", "React", "TypeScript", "Python", "Rust", "Go",
        ],
    },
    {
        "name": "general_business",
        "weight": 0.20,
        "topics": [
            "quarterly business review", "hiring process update",
            "client presentation prep", "budget planning",
            "team restructuring", "vendor negotiation",
            "marketing campaign results", "product roadmap",
            "customer feedback analysis", "partnership discussion",
            "office logistics", "training session planning",
            "performance review notes", "project status update",
        ],
        "jargon": [
            "Q1", "Q2", "Q3", "Q4", "KPI", "OKR", "ROI",
            "NPS", "ARR", "MRR", "churn rate", "pipeline",
        ],
    },
    {
        "name": "casual_conversational",
        "weight": 0.15,
        "topics": [
            "planning lunch", "weekend plans", "commute update",
            "quick favor request", "sharing a recommendation",
            "running late notification", "thank you note",
            "birthday wishes", "team outing coordination",
            "equipment borrowing", "parking situation",
        ],
        "jargon": [],
    },
    {
        "name": "medical_health",
        "weight": 0.10,
        "topics": [
            "patient admission note", "medication adjustment",
            "lab results review", "surgical planning",
            "discharge instructions", "referral to specialist",
            "imaging findings", "treatment plan update",
            "clinical trial discussion", "symptom assessment",
        ],
        "jargon": [
            "hypertension", "diabetes", "metformin", "atorvastatin",
            "MRI", "CT scan", "echocardiogram", "creatinine",
            "hemoglobin", "A1C", "ejection fraction", "tachycardia",
        ],
    },
    {
        "name": "legal_compliance",
        "weight": 0.08,
        "topics": [
            "contract review notes", "compliance audit findings",
            "motion filing", "discovery process", "settlement discussion",
            "regulatory update", "NDA terms", "IP dispute",
            "data privacy assessment", "employment law question",
        ],
        "jargon": [
            "plaintiff", "defendant", "subpoena", "deposition",
            "GDPR", "HIPAA", "SOX", "indemnification", "arbitration",
            "statute of limitations", "due diligence", "force majeure",
        ],
    },
    {
        "name": "finance",
        "weight": 0.07,
        "topics": [
            "quarterly earnings review", "budget variance analysis",
            "investment thesis", "risk assessment", "cash flow forecast",
            "cost reduction initiative", "revenue projection",
            "audit preparation", "tax planning", "capital allocation",
        ],
        "jargon": [
            "EBITDA", "P/E ratio", "DCF", "IRR", "NPV", "WACC",
            "accounts receivable", "depreciation", "amortization",
            "gross margin", "operating leverage", "SOFR",
        ],
    },
    {
        "name": "academic_research",
        "weight": 0.05,
        "topics": [
            "study methodology", "data analysis results",
            "grant proposal draft", "peer review response",
            "conference presentation prep", "literature review notes",
            "IRB submission", "statistical analysis plan",
        ],
        "jargon": [
            "p-value", "confidence interval", "regression",
            "meta-analysis", "randomized controlled trial",
            "effect size", "Cohen's d", "IRB", "informed consent",
        ],
    },
    {
        "name": "technical_other",
        "weight": 0.05,
        "topics": [
            "hardware setup", "network configuration",
            "data science pipeline", "ML model evaluation",
            "IoT sensor deployment", "embedded system debugging",
        ],
        "jargon": [
            "FPGA", "VLAN", "subnet", "latency", "throughput",
            "TensorFlow", "PyTorch", "ONNX", "batch size", "epoch",
        ],
    },
    {
        "name": "creative_content",
        "weight": 0.05,
        "topics": [
            "blog post outline", "social media copy",
            "email newsletter draft", "video script notes",
            "podcast episode planning", "marketing copy review",
        ],
        "jargon": [],
    },
]

PERSONAS = [
    {"name": "senior_engineer", "desc": "Confident, concise, uses technical jargon fluently. Few fillers."},
    {"name": "junior_developer", "desc": "Hesitant, many fillers (uh, um), seeks confirmation ('right?'). Restarts thoughts often."},
    {"name": "manager", "desc": "Strategic language, focuses on action items and delegation. Medium fillers."},
    {"name": "non_native_speaker", "desc": "Occasional grammar errors (article dropping, preposition mistakes). Simpler vocabulary. Medium fillers."},
    {"name": "fast_talker", "desc": "Rapid speech with many false starts, corrections, and incomplete thoughts. High energy."},
    {"name": "deliberate_speaker", "desc": "Careful, measured speech. Very few fillers. Pauses represented by 'um'. Precise word choice."},
    {"name": "domain_expert", "desc": "Heavy jargon, assumes audience knowledge. Few fillers but complex sentence structure."},
    {"name": "casual_dictator", "desc": "Short bursts, informal, uses dictation commands (period, comma). Lots of contractions."},
    {"name": "nervous_presenter", "desc": "Very high filler rate, lots of 'you know' and 'basically'. Repeats key phrases."},
    {"name": "executive", "desc": "Authoritative, concise directives. Very few fillers. Expects action."},
]

CATEGORIES = [
    {"name": "filler_removal", "weight": 0.12, "description": "Remove verbal fillers (uh, um, uhm, er, ah) from the transcript."},
    {"name": "crutch_words", "weight": 0.08, "description": "Remove crutch words/phrases (basically, you know, I mean, honestly, literally, anyway) and filler uses of like/so/okay/yeah/right."},
    {"name": "self_correction", "weight": 0.14, "description": "Speaker corrects themselves mid-sentence. Delete original, keep only correction."},
    {"name": "false_start", "weight": 0.08, "description": "Speaker starts a thought, abandons it, restarts. Remove the abandoned portion."},
    {"name": "grammar", "weight": 0.07, "description": "Fix spoken grammar: gonna→going to, its→it's, subject-verb agreement, run-on sentences."},
    {"name": "misheard_words", "weight": 0.07, "description": "ASR misheard domain terms. Fix phonetically plausible errors."},
    {"name": "phonetic_errors", "weight": 0.06, "description": "Common 'sounds like' ASR mistakes: homophones, near-homophones, phonetic confusions."},
    {"name": "dictation_commands", "weight": 0.07, "description": "Convert spoken punctuation: 'period'→'.', 'comma'→',', 'slash'→'/', etc."},
    {"name": "list_formatting", "weight": 0.05, "description": "Convert spoken numbered items into formatted numbered lists."},
    {"name": "preserve_wording", "weight": 0.10, "description": "Input is clean/near-clean. Output identical except punctuation/capitalization."},
    {"name": "mixed", "weight": 0.08, "description": "Multiple disfluency types combined in one passage."},
    {"name": "long_transcript", "weight": 0.08, "description": "Long continuous dictation (300-1000 words) with scattered disfluencies. Must preserve ALL content."},
    {"name": "adversarial", "weight": 0.04, "description": "Words that look like fillers but are used meaningfully. Must NOT be removed."},
]

DIFFICULTIES = [
    {"name": "easy", "weight": 0.30, "desc": "Single disfluency, 5-25 words"},
    {"name": "medium", "weight": 0.45, "desc": "2-3 disfluencies, 15-60 words"},
    {"name": "hard", "weight": 0.25, "desc": "4+ disfluencies, 40-250 words"},
]

# ---------------------------------------------------------------------------
# Prompt Templates
# ---------------------------------------------------------------------------

SYSTEM_PROMPT = """You are a training data generator for a speech-to-text transcript cleanup model. You generate realistic (raw_transcript, cleaned_transcript) pairs.

CRITICAL RULES for the "raw" field:
- ALL LOWERCASE, no capital letters
- NO PUNCTUATION: no periods, commas, question marks, exclamation points, colons, semicolons, hyphens, or apostrophes
- Numbers appear as digits: "500" not "five hundred" (ASR outputs digits)
- Contractions are split: "dont" not "don't", "im" not "I'm", "weve" not "we've"
- Compound words are separated: "web socket" not "websocket" (ASR splits them)

CRITICAL RULES for the "clean" field:
- Proper punctuation, capitalization, and formatting
- Fix contractions: "dont" → "don't", "im" → "I'm"
- NEVER paraphrase, summarize, or add content not in "raw"
- For self-corrections: contains ONLY the final/corrected version
- For preserve_wording/adversarial: identical to "raw" except punctuation/caps

Output ONLY a valid JSON array, no markdown fencing, no commentary."""


def build_generation_prompt(category: dict, domain: dict, persona: dict, difficulty: dict, batch_size: int = BATCH_SIZE) -> str:
    """Build a prompt for generating a batch of training pairs."""
    topic = random.choice(domain["topics"])
    jargon_hint = ""
    if domain["jargon"]:
        sample_jargon = random.sample(domain["jargon"], min(5, len(domain["jargon"])))
        jargon_hint = f"\nDomain terminology to optionally use: {', '.join(sample_jargon)}"

    cat_instructions = _get_category_instructions(category["name"])

    return f"""Generate exactly {batch_size} transcript cleanup training pairs.

CATEGORY: {category["name"]} — {category["description"]}
DOMAIN: {domain["name"]} — Topic: {topic}{jargon_hint}
SPEAKER: {persona["name"]} — {persona["desc"]}
DIFFICULTY: {difficulty["name"]} — {difficulty["desc"]}

{cat_instructions}

Output a JSON array of objects. Each object has "raw" and "clean" fields:
[{{"raw": "uh the server is uh running low on memory", "clean": "The server is running low on memory."}}, ...]

Generate {batch_size} DIVERSE examples. Vary sentence structure, vocabulary, and content."""


def _get_category_instructions(category_name: str) -> str:
    """Return category-specific generation instructions."""
    instructions = {
        "filler_removal": """FILLER REMOVAL:
- Insert fillers (uh, um, uhm, er, ah, hmm) at natural positions
- Vary density: 1 filler to 4+ per sentence
- Clean removes ALL fillers, changes nothing else
Examples:
  "i uh need you to uh send me the report" → "I need you to send me the report."
  "the um database connection is um timing out" → "The database connection is timing out."
  "uh can you uh check if the server is uh responding" → "Can you check if the server is responding?" """,

        "crutch_words": """CRUTCH WORDS:
- Insert: basically, you know, I mean, honestly, literally, anyway
- Also filler uses of: like, so, okay, yeah, right
- Clean removes crutch words, preserves all meaningful content
Examples:
  "so basically what we need to do is refactor the auth module" → "What we need to do is refactor the auth module."
  "okay so the thing is basically were running out of disk space" → "We're running out of disk space."
  "i mean honestly the design is pretty solid" → "The design is pretty solid." """,

        "self_correction": """SELF-CORRECTION (hardest category — be very precise):
- Speaker says X, then corrects to Y using: wait, actually, no, scratch that, sorry, I mean, or rather, hold on
- Patterns: simple swap, value correction, target change, full rethink, double correction, correction with reasoning
- Clean contains ONLY the final/corrected version — DELETE everything before the correction marker
- Example: "use redis wait no memcached is better" → "Use Memcached."
- Example: "set to 100 actually 500 for production" → "Set to 500 for production."
- Example: "deploy to dev no staging no production" → "Deploy to production." """,

        "false_start": """FALSE START:
- Speaker starts, abandons, restarts: word repetition, phrase restart, synonym restart
- Clean keeps only the completed version
Examples:
  "the the server needs to be restarted" → "The server needs to be restarted."
  "we need to we should probably add input validation" → "We should probably add input validation."
  "the function the method takes two parameters" → "The method takes two parameters." """,

        "grammar": """GRAMMAR:
- Spoken patterns: gonna, wanna, gotta, should of, its/it's confusion
- Subject-verb agreement, run-on sentences, missing articles
- Clean fixes grammar to standard written English
Examples:
  "we gonna need more time for this feature" → "We're going to need more time for this feature."
  "the tests is failing on the ci pipeline" → "The tests are failing on the CI pipeline."
  "me and him will work on the migration" → "He and I will work on the migration." """,

        "misheard_words": """MISHEARD WORDS:
- ASR misheard domain terms: compound splits, phonetic subs, acronym mishearing
- Context must make the correct term clear. Make mishearings phonetically plausible.
Examples:
  "deploy the app to the cube er netties cluster" → "Deploy the app to the Kubernetes cluster."
  "the web socket connection keeps dropping" → "The WebSocket connection keeps dropping."
  "we should use oh auth two for authentication" → "We should use OAuth 2.0 for authentication."
  "the jason payload is malformed" → "The JSON payload is malformed." """,

        "dictation_commands": """DICTATION COMMANDS:
- Spoken punctuation: period→., comma→,, slash→/, question mark→?, exclamation point→!, colon→:, dot→.
- Clean converts to actual punctuation. Mix with regular content.
Examples:
  "send the email to john period" → "Send the email to John."
  "dear team comma i wanted to share an update period" → "Dear team, I wanted to share an update."
  "the url is api dot example dot com slash v2 slash users" → "The URL is api.example.com/v2/users."
  "is the deployment done question mark" → "Is the deployment done?" """,

        "list_formatting": """LIST FORMATTING:
- Spoken numbered items: first/second/third, one/two/three, step one/step two
- Clean formats as numbered list. 2-7 items. Only convert CLEAR lists.
Examples:
  "the priorities are one fix the login bug two add search three update docs" → "The priorities are:\\n1. Fix the login bug\\n2. Add search\\n3. Update docs"
  "step one open the terminal step two run the build" → "1. Open the terminal\\n2. Run the build" """,

        "preserve_wording": """PRESERVE WORDING (prevents over-correction):
- Generate clean, well-formed text needing MINIMAL changes
- Raw is lowercase without punctuation. Clean adds ONLY punctuation and capitalization.
- IMPORTANT: The raw text should be CLEAN SPEECH with NO fillers (no uh, um, er). This category tests that the model does NOT over-edit clean input.
- Include emphasis words: really, very, definitely, absolutely
- Include preserved phrases: "go ahead and", "I want you to", "a lot of", "kind of"
- Include short commands: "ship it", "merge the PR"
- Do NOT put fillers in the raw text for this category
Examples:
  "lets go ahead and deploy this to staging" → "Let's go ahead and deploy this to staging."
  "i really think we should prioritize the security audit" → "I really think we should prioritize the security audit."
  "ship it" → "Ship it."
  "the client is very happy with the results" → "The client is very happy with the results."
  "we have a lot of work to do before the deadline" → "We have a lot of work to do before the deadline." """,

        "mixed": """MIXED DISFLUENCIES:
- Combine 2-5 types: fillers + self-correction, crutch words + false starts + grammar
- Should feel like real speech, not artificial stacking
- Clean addresses ALL disfluencies
Examples:
  "so uh basically the the api is um throttling our requests because we exceeded the rate limit" → "The API is throttling our requests because we exceeded the rate limit."
  "i think we should uh use kafka no wait actually rabbitmq would be simpler for our use case" → "I think we should use RabbitMQ. It would be simpler for our use case."
  "um we we gotta ship this by end of sprint or were gonna miss the release" → "We have to ship this by end of sprint, or we're going to miss the release." """,

        "adversarial": """ADVERSARIAL (what NOT to remove):
- "like" as verb: "i like this design" → "I like this design."
- "actually" as adverb: "it actually works" → "It actually works."
- "no" as negation: "no we should not" → "No, we should not."
- "wait" as command: "wait for the build" → "Wait for the build."
- "period" as noun: "the trial period" → "The trial period."
- Intentional repetition: "really really important" → "Really really important."
- Clean preserves ALL these — only adds punctuation""",

        "phonetic_errors": """PHONETIC / "SOUNDS LIKE" ASR ERRORS:
Generate examples where the ASR system confused words that sound similar.
Types to cover:
- Homophones: "their/there/they're", "its/it's", "your/you're", "to/too/two", "then/than", "affect/effect", "accept/except", "weather/whether"
- Near-homophones: "specific" heard as "pacific", "espresso" as "expresso", "supposedly" as "supposably", "library" as "libary", "probably" as "prolly", "definitely" as "defiantly"
- Phonetic splits: "a lot" heard as "allot", "all right" as "aright", "in fact" as "infact"
- Common mishearings: "for all intents and purposes" heard as "for all intensive purposes", "nip it in the bud" as "nip it in the butt", "couldn't care less" as "could care less"
- Tech phonetic: "OAuth" as "oh auth", "SQL" as "sequel", "GUI" as "gooey", "AJAX" as "a jacks", "regex" as "rej ex"
Examples:
  "the weather were going to deploy depends on the test results" → "Whether we're going to deploy depends on the test results."
  "its defiantly the right approach for all intensive purposes" → "It's definitely the right approach, for all intents and purposes."
  "there going to except the pull request than merge it" → "They're going to accept the pull request, then merge it." """,

        "long_transcript": """LONG TRANSCRIPT (300-1000 words):
Generate ONE long, realistic transcript pair. This simulates someone dictating for 2-5 minutes continuously.

CRITICAL REQUIREMENTS:
- The RAW transcript must be 300-1000 words of CONTINUOUS speech
- Include 10-30 disfluencies scattered NATURALLY throughout (not clustered)
- Include at least 2 self-corrections, 5+ fillers, 2+ crutch words, 1+ false start
- Cover 2-4 different topics or subtopics (like a real meeting update)
- The CLEAN version must preserve ALL meaningful content — output should be 80-95% of raw length
- Do NOT summarize, condense, or restructure — only remove disfluencies and fix formatting
- Include domain-specific terminology
- Make it sound like REAL unrehearsed speech, not written text read aloud

Output format: ONE JSON object (not an array):
{"raw": "very long lowercase no punctuation text...", "clean": "Very long properly formatted text..."}

IMPORTANT: The clean version must be nearly as long as the raw version. If raw is 500 words, clean should be 400-475 words. Never produce a clean version that is less than 75% of the raw length.""",
    }
    return instructions.get(category_name, "Generate realistic examples for this category.")


# ---------------------------------------------------------------------------
# Data classes
# ---------------------------------------------------------------------------

@dataclass
class Sample:
    raw: str
    clean: str
    category: str
    domain: str
    persona: str
    difficulty: str
    batch_id: str
    valid: bool = True
    rejection_reason: str = ""

    def to_train_format(self) -> dict:
        return {"input": self.raw, "output": self.clean}

    def to_metadata(self) -> dict:
        return asdict(self)

    def fingerprint(self) -> str:
        """Hash raw text for deduplication. Same input → same output expected."""
        return hashlib.md5(self.raw.encode()).hexdigest()


# ---------------------------------------------------------------------------
# Validation
# ---------------------------------------------------------------------------

FILLER_PATTERN = re.compile(r'\b(uh|um|uhm|er|ah|hmm|hm|mhm)\b', re.IGNORECASE)
CORRECTION_MARKERS = re.compile(r'\b(wait|actually|no|scratch that|sorry|i mean|i meant|or rather|hold on|oh wait|let me rephrase|rather)\b', re.IGNORECASE)
RAW_PUNCTUATION = re.compile(r"[!?,;:]")  # sentence punctuation that should NOT be in raw field
# Note: periods, apostrophes, and hyphens are ALLOWED in raw because:
#   - Periods appear in numbers/versions: "0.001", "v2.1", "192.168.1.1"
#   - Apostrophes appear in contractions: "we're", "don't", "it's" (ASR produces these)
#   - Hyphens appear in compound words: "real-time", "e-commerce"


def validate_sample(sample: Sample) -> Sample:
    """Apply validation rules. Sets sample.valid=False with reason if invalid."""
    raw, clean = sample.raw.strip(), sample.clean.strip()

    if not raw or not clean:
        sample.valid, sample.rejection_reason = False, "empty_field"
        return sample

    raw_words = len(raw.split())
    clean_words = len(clean.split())

    if raw_words < MIN_INPUT_WORDS:
        sample.valid, sample.rejection_reason = False, f"raw_too_short ({raw_words}w)"
        return sample
    if raw_words > MAX_INPUT_WORDS:
        sample.valid, sample.rejection_reason = False, f"raw_too_long ({raw_words}w)"
        return sample
    if clean_words < MIN_OUTPUT_WORDS:
        sample.valid, sample.rejection_reason = False, f"clean_too_short ({clean_words}w)"
        return sample
    if clean_words > MAX_OUTPUT_WORDS:
        sample.valid, sample.rejection_reason = False, f"clean_too_long ({clean_words}w)"
        return sample

    # Length ratio
    ratio = len(clean) / max(1, len(raw))
    if ratio < MIN_LENGTH_RATIO and sample.category != "self_correction":
        sample.valid, sample.rejection_reason = False, f"ratio_low ({ratio:.2f})"
        return sample
    if ratio > MAX_LENGTH_RATIO and sample.category not in ("preserve_wording", "adversarial", "dictation_commands", "grammar"):
        sample.valid, sample.rejection_reason = False, f"ratio_high ({ratio:.2f})"
        return sample

    # Raw should be mostly lowercase (allow some for acronyms like PR, JWT, MRI)
    uppercase_ratio = sum(1 for c in raw if c.isupper()) / max(1, len(raw))
    if uppercase_ratio > 0.15:
        sample.valid, sample.rejection_reason = False, f"raw_not_lowercase ({uppercase_ratio:.0%})"
        return sample

    # Raw should not contain sentence punctuation (ASR doesn't produce it)
    punct_count = len(RAW_PUNCTUATION.findall(raw))
    if punct_count > 0 and sample.category != "dictation_commands":
        sample.valid, sample.rejection_reason = False, f"raw_has_punctuation ({punct_count})"
        return sample

    # Clean should start uppercase (for non-trivial text)
    if clean_words > 2 and clean[0].islower():
        sample.valid, sample.rejection_reason = False, "clean_no_cap"
        return sample

    # Clean should end with punctuation (except lists)
    if clean[-1] not in '.!?\n' and sample.category != "list_formatting":
        sample.valid, sample.rejection_reason = False, "clean_no_punct"
        return sample

    # No fillers in clean (except adversarial/preserve categories)
    if sample.category not in ("adversarial", "preserve_wording"):
        if FILLER_PATTERN.findall(clean):
            sample.valid, sample.rejection_reason = False, "fillers_in_clean"
            return sample

    # Preserve/adversarial: content should be very similar
    if sample.category in ("preserve_wording", "adversarial"):
        raw_ws = set(re.findall(r'\w+', raw.lower()))
        clean_ws = set(re.findall(r'\w+', clean.lower()))
        if raw_ws and clean_ws:
            overlap = len(raw_ws & clean_ws) / max(1, len(raw_ws))
            if overlap < 0.85:
                sample.valid, sample.rejection_reason = False, f"content_mismatch ({overlap:.0%})"
                return sample

    # Self-correction: raw should contain at least one correction marker
    if sample.category == "self_correction":
        if not CORRECTION_MARKERS.search(raw):
            sample.valid, sample.rejection_reason = False, "no_correction_marker"
            return sample

    # Semantic relevance: clean text words should largely come from raw
    # (catches hallucinated clean text unrelated to raw)
    if sample.category not in ("preserve_wording", "adversarial", "grammar", "misheard_words"):
        stopwords = {"the", "a", "an", "is", "are", "was", "were", "be", "to", "of", "and", "in", "for", "on", "with", "it", "that", "this", "i"}
        filler_words = {"uh", "um", "uhm", "er", "ah", "hmm", "hm", "mhm"}
        raw_content = set(re.findall(r'\w+', raw.lower())) - stopwords - filler_words
        clean_content = set(re.findall(r'\w+', clean.lower())) - stopwords
        if raw_content and clean_content:
            overlap = len(raw_content & clean_content) / max(1, len(clean_content))
            if overlap < 0.4:
                sample.valid, sample.rejection_reason = False, f"semantic_mismatch ({overlap:.0%})"
                return sample

    return sample


# ---------------------------------------------------------------------------
# API Client
# ---------------------------------------------------------------------------

def _is_bedrock_url(base_url: str) -> bool:
    """Check if this is a Bedrock Converse API endpoint."""
    return "bedrock-runtime" in base_url and "amazonaws.com" in base_url


async def _call_bedrock(session: aiohttp.ClientSession, base_url: str, model: str,
                        api_key: str, system: str, user: str) -> Optional[str]:
    """Call AWS Bedrock Converse API (not OpenAI-compatible)."""
    url = f"{base_url}/model/{model}/converse"
    headers = {
        "Content-Type": "application/json",
        "Authorization": f"Bearer {api_key}",
    }
    payload = {
        "system": [{"text": system}],
        "messages": [{"role": "user", "content": [{"text": user}]}],
        "inferenceConfig": {
            "maxTokens": MAX_TOKENS,
            "temperature": TEMPERATURE,
            # Note: Claude on Bedrock doesn't allow both temperature and topP
        },
    }

    for attempt in range(3):
        try:
            async with session.post(url, json=payload, headers=headers,
                                    timeout=aiohttp.ClientTimeout(total=300)) as resp:
                if resp.status == 200:
                    data = await resp.json()
                    content_blocks = data.get("output", {}).get("message", {}).get("content", [])
                    if content_blocks:
                        return content_blocks[0].get("text", "")
                    return None
                elif resp.status == 429:
                    wait = 2 ** attempt + random.random()
                    logging.warning(f"Bedrock rate limited, waiting {wait:.1f}s...")
                    await asyncio.sleep(wait)
                elif resp.status >= 500:
                    wait = 2 ** attempt + random.random()
                    logging.warning(f"Bedrock server error {resp.status}, retrying...")
                    await asyncio.sleep(wait)
                else:
                    body = await resp.text()
                    logging.error(f"Bedrock error {resp.status}: {body[:200]}")
                    return None
        except asyncio.TimeoutError:
            logging.warning(f"Bedrock timeout on attempt {attempt + 1}/3")
            await asyncio.sleep(2 ** attempt)
        except Exception as e:
            logging.error(f"Bedrock request error: {e}")
            await asyncio.sleep(2 ** attempt)
    return None


async def _call_openai(session: aiohttp.ClientSession, base_url: str, model: str,
                       api_key: str, system: str, user: str) -> Optional[str]:
    """Call OpenAI-compatible API (vLLM, OpenRouter, etc.)."""
    headers = {"Content-Type": "application/json"}
    if api_key:
        headers["Authorization"] = f"Bearer {api_key}"

    payload = {
        "model": model,
        "messages": [
            {"role": "system", "content": system},
            {"role": "user", "content": user},
        ],
        "temperature": TEMPERATURE,
        "top_p": TOP_P,
        "max_tokens": MAX_TOKENS,
        "chat_template_kwargs": {"enable_thinking": False},
        "reasoning": {"effort": "none"},
    }

    url = f"{base_url}/chat/completions"

    for attempt in range(3):
        try:
            async with session.post(url, json=payload, headers=headers,
                                    timeout=aiohttp.ClientTimeout(total=300)) as resp:
                if resp.status == 200:
                    data = await resp.json()
                    msg = data["choices"][0]["message"]
                    content = msg.get("content")
                    if content:
                        return content
                    reasoning = msg.get("reasoning")
                    if reasoning:
                        logging.debug("content was None, using reasoning field")
                        return reasoning
                    return None
                elif resp.status == 429:
                    wait = 2 ** attempt + random.random()
                    logging.warning(f"Rate limited, waiting {wait:.1f}s...")
                    await asyncio.sleep(wait)
                elif resp.status >= 500:
                    wait = 2 ** attempt + random.random()
                    logging.warning(f"Server error {resp.status}, retrying...")
                    await asyncio.sleep(wait)
                else:
                    body = await resp.text()
                    logging.error(f"API error {resp.status}: {body[:200]}")
                    return None
        except asyncio.TimeoutError:
            logging.warning(f"Timeout on attempt {attempt + 1}/3")
            await asyncio.sleep(2 ** attempt)
        except Exception as e:
            logging.error(f"Request error: {e}")
            await asyncio.sleep(2 ** attempt)

    return None


async def call_llm(session: aiohttp.ClientSession, base_url: str, model: str,
                   api_key: str, system: str, user: str) -> Optional[str]:
    """Unified LLM call — routes to Bedrock Converse or OpenAI-compatible API."""
    if _is_bedrock_url(base_url):
        return await _call_bedrock(session, base_url, model, api_key, system, user)
    else:
        return await _call_openai(session, base_url, model, api_key, system, user)


def parse_json_response(text: str) -> list[dict]:
    """Extract a JSON array from the LLM response."""
    if not text:
        return []

    text = text.strip()
    # Strip markdown fencing
    if text.startswith("```"):
        text = re.sub(r'^```(?:json)?\s*\n?', '', text)
        text = re.sub(r'\n?```\s*$', '', text)
        text = text.strip()

    # Direct parse
    try:
        result = json.loads(text)
        if isinstance(result, list):
            return result
        if isinstance(result, dict):
            return [result]
    except json.JSONDecodeError:
        pass

    # Find JSON array with bracket counting (handles nested structures)
    start = text.find('[')
    if start != -1:
        depth = 0
        found = False
        for i in range(start, len(text)):
            if text[i] == '[':
                depth += 1
            elif text[i] == ']':
                depth -= 1
                if depth == 0:
                    found = True
                    try:
                        return json.loads(text[start:i + 1])
                    except json.JSONDecodeError:
                        break
        if not found:
            logging.debug(f"JSON array not closed — possible truncation (response length: {len(text)} chars)")

    # Fallback: line-by-line JSONL
    results = []
    for line in text.split('\n'):
        line = line.strip().rstrip(',')
        if line.startswith('{'):
            try:
                results.append(json.loads(line))
            except json.JSONDecodeError:
                pass
    return results


# ---------------------------------------------------------------------------
# Weighted random selection
# ---------------------------------------------------------------------------

def weighted_choice(items: list[dict], key: str = "weight") -> dict:
    return random.choices(items, weights=[i[key] for i in items], k=1)[0]


# ---------------------------------------------------------------------------
# Generation worker
# ---------------------------------------------------------------------------

async def generate_batch(
    session: aiohttp.ClientSession,
    base_url: str, model: str, api_key: str,
    batch_id: str, category_filter: Optional[str] = None,
) -> list[Sample]:
    """Generate a single batch of samples."""
    cat = None
    if category_filter:
        cat = next((c for c in CATEGORIES if c["name"] == category_filter), None)
        if not cat:
            logging.warning(f"Category '{category_filter}' not found, using weighted random")
    if not cat:
        cat = weighted_choice(CATEGORIES)

    domain = weighted_choice(DOMAINS)
    persona = random.choice(PERSONAS)
    difficulty = weighted_choice(DIFFICULTIES)

    # Long transcripts get 1 sample per batch (they're big); others get 10
    batch_size = 1 if cat["name"] == "long_transcript" else BATCH_SIZE
    prompt = build_generation_prompt(cat, domain, persona, difficulty, batch_size)
    response = await call_llm(session, base_url, model, api_key, SYSTEM_PROMPT, prompt)

    if not response:
        return []

    # Long transcripts return a single JSON object, not an array
    if cat["name"] == "long_transcript":
        try:
            item = json.loads(response.strip().strip('`').removeprefix('json').strip())
            if isinstance(item, dict):
                parsed = [item]
            elif isinstance(item, list):
                parsed = item
            else:
                parsed = []
        except json.JSONDecodeError:
            parsed = parse_json_response(response)
    else:
        parsed = parse_json_response(response)
    samples = []

    for item in parsed:
        raw = item.get("raw", "").strip()
        clean = item.get("clean", "").strip()
        if raw and clean:
            s = Sample(
                raw=raw, clean=clean,
                category=cat["name"], domain=domain["name"],
                persona=persona["name"], difficulty=difficulty["name"],
                batch_id=batch_id,
            )
            s = validate_sample(s)
            samples.append(s)

    return samples


# ---------------------------------------------------------------------------
# Main generation loop — bounded worker pool pattern
# ---------------------------------------------------------------------------

async def run_generation(
    base_url: str, model: str, api_key: str,
    target: int, concurrency: int,
    output_dir: Path,
    category: Optional[str] = None,
    resume: bool = False,
):
    """Main generation loop using a bounded worker pool (no task overshoot)."""

    output_dir.mkdir(parents=True, exist_ok=True)
    train_file = output_dir / "train.jsonl"
    meta_file = output_dir / "metadata.jsonl"
    rejected_file = output_dir / "rejected.jsonl"
    checkpoint_file = output_dir / ".checkpoint.json"

    # State
    total_valid = 0
    total_generated = 0
    total_rejected = 0
    total_duplicate = 0
    batch_counter = 0
    seen_fingerprints: set[str] = set()
    cat_counts: Counter = Counter()
    domain_counts: Counter = Counter()

    # Resume support
    if resume and checkpoint_file.exists():
        with open(checkpoint_file) as f:
            ckpt = json.load(f)
        total_rejected = ckpt.get("total_rejected", 0)
        total_duplicate = ckpt.get("total_duplicate", 0)
        batch_counter = ckpt.get("batch_counter", 0)
        cat_counts = Counter(ckpt.get("category_counts", {}))
        domain_counts = Counter(ckpt.get("domain_counts", {}))
        logging.info(f"Loaded checkpoint: batch_counter={batch_counter}")

    if resume and train_file.exists():
        with open(train_file) as f:
            for line in f:
                if line.strip():
                    total_valid += 1
                    d = json.loads(line)
                    seen_fingerprints.add(hashlib.md5(d["input"].encode()).hexdigest())
        logging.info(f"Resuming from {total_valid:,} existing samples")
        total_generated = total_valid + total_rejected + total_duplicate

    start_time = time.time()
    start_valid = total_valid

    # Open files
    mode = "a" if resume else "w"
    train_fh = open(train_file, mode)
    meta_fh = open(meta_file, mode)
    rejected_fh = open(rejected_file, mode)

    # Bounded worker pool using asyncio.Queue
    queue: asyncio.Queue[Optional[str]] = asyncio.Queue()
    stop_event = asyncio.Event()

    async def worker(worker_id: int):
        nonlocal total_valid, total_generated, total_rejected, total_duplicate, batch_counter

        while not stop_event.is_set():
            batch_id = await queue.get()
            if batch_id is None:  # poison pill
                queue.task_done()
                break

            try:
                samples = await generate_batch(
                    session, base_url, model, api_key,
                    batch_id=batch_id, category_filter=category,
                )
                for s in samples:
                    total_generated += 1
                    fp = s.fingerprint()

                    if not s.valid:
                        total_rejected += 1
                        rejected_fh.write(json.dumps({"raw": s.raw[:200], "clean": s.clean[:200], "reason": s.rejection_reason, "cat": s.category}) + "\n")
                        logging.debug(f"Rejected ({s.rejection_reason}): raw={s.raw[:80]!r}")
                        continue

                    if fp in seen_fingerprints:
                        total_duplicate += 1
                        continue

                    seen_fingerprints.add(fp)
                    total_valid += 1
                    cat_counts[s.category] += 1
                    domain_counts[s.domain] += 1

                    train_fh.write(json.dumps(s.to_train_format()) + "\n")
                    meta_fh.write(json.dumps(s.to_metadata()) + "\n")

                # Flush after every batch
                train_fh.flush()
                meta_fh.flush()
                rejected_fh.flush()

                # Check if we've reached the target
                if total_valid >= target:
                    stop_event.set()

            except Exception as e:
                logging.error(f"Worker {worker_id} error: {e}")
            finally:
                queue.task_done()

    try:
        connector = aiohttp.TCPConnector(limit=concurrency * 2, force_close=False)
        async with aiohttp.ClientSession(connector=connector) as session:
            remaining = target - total_valid
            logging.info(f"Target: {target:,} | Current: {total_valid:,} | Need: {remaining:,}")
            logging.info(f"Concurrency: {concurrency} workers | Batch size: {BATCH_SIZE}")

            # Start worker tasks
            workers = [asyncio.create_task(worker(i)) for i in range(concurrency)]

            # Feed batches to the queue with time-based stall detection
            last_report = time.time()
            last_progress_time = time.time()
            last_valid_snapshot = total_valid
            STALL_TIMEOUT_S = 180  # 3 min with no new valid samples → abort

            while total_valid < target and not stop_event.is_set():
                batch_counter += 1
                batch_id = f"B{batch_counter:06d}"
                await queue.put(batch_id)

                # Backpressure: don't enqueue too far ahead of workers
                while queue.qsize() > concurrency * 2 and not stop_event.is_set():
                    await asyncio.sleep(0.5)

                # Time-based stall detection (checked during backpressure waits too)
                if total_valid > last_valid_snapshot:
                    last_valid_snapshot = total_valid
                    last_progress_time = time.time()
                elif time.time() - last_progress_time > STALL_TIMEOUT_S:
                    logging.error(
                        f"STALL ABORT: No new valid samples in {STALL_TIMEOUT_S}s. "
                        f"Generated={total_generated} Rejected={total_rejected}. "
                        f"Model may not be producing valid output for this config."
                    )
                    stop_event.set()
                    break

                # Progress reporting every 15 seconds
                now = time.time()
                if now - last_report > 15:
                    elapsed = now - start_time
                    new_valid = total_valid - start_valid
                    rate = new_valid / elapsed if elapsed > 0 else 0
                    eta = (target - total_valid) / rate if rate > 0 else 0
                    valid_rate = total_valid / max(1, total_generated) * 100
                    logging.info(
                        f"Progress: {total_valid:,}/{target:,} ({total_valid / max(1, target) * 100:.1f}%) | "
                        f"Rate: {rate:.1f}/s | ETA: {eta / 60:.0f}min | "
                        f"Valid: {valid_rate:.0f}% | Rejected: {total_rejected:,} | Dupes: {total_duplicate:,}"
                    )
                    last_report = now

            # Send poison pills to stop workers
            for _ in range(concurrency):
                await queue.put(None)

            # Wait for all workers to finish
            await asyncio.gather(*workers, return_exceptions=True)

    finally:
        train_fh.close()
        meta_fh.close()
        rejected_fh.close()

        # Save checkpoint
        checkpoint = {
            "total_valid": total_valid,
            "total_generated": total_generated,
            "total_rejected": total_rejected,
            "total_duplicate": total_duplicate,
            "batch_counter": batch_counter,
            "category_counts": dict(cat_counts),
            "domain_counts": dict(domain_counts),
            "target": target,
            "timestamp": time.time(),
        }
        with open(checkpoint_file, "w") as f:
            json.dump(checkpoint, f, indent=2)

    # Final summary
    elapsed = time.time() - start_time
    new_valid = total_valid - start_valid
    logging.info("=" * 70)
    logging.info("GENERATION COMPLETE")
    logging.info("=" * 70)
    logging.info(f"  Total valid:    {total_valid:,}")
    logging.info(f"  New this run:   {new_valid:,}")
    logging.info(f"  Rejected:       {total_rejected:,}")
    logging.info(f"  Duplicates:     {total_duplicate:,}")
    logging.info(f"  Valid rate:     {total_valid / max(1, total_generated) * 100:.1f}%")
    logging.info(f"  Time elapsed:   {elapsed / 60:.1f} minutes")
    logging.info(f"  Rate:           {new_valid / max(1, elapsed):.1f} samples/sec")
    logging.info(f"  Output:         {train_file}")
    logging.info("")
    logging.info("  Category distribution:")
    for cat_name, cnt in sorted(cat_counts.items(), key=lambda x: -x[1]):
        logging.info(f"    {cat_name:25s}: {cnt:6,} ({cnt / max(1, total_valid) * 100:.1f}%)")
    logging.info("")
    logging.info("  Domain distribution:")
    for dom, cnt in sorted(domain_counts.items(), key=lambda x: -x[1]):
        logging.info(f"    {dom:25s}: {cnt:6,} ({cnt / max(1, total_valid) * 100:.1f}%)")

    # Create train/val split
    logging.info("")
    logging.info("Creating train/val split (90/10)...")
    all_samples = []
    with open(train_file) as f:
        for line in f:
            if line.strip():
                all_samples.append(line.strip())
    random.shuffle(all_samples)
    split_idx = int(len(all_samples) * 0.9)
    train_split = all_samples[:split_idx]
    val_split = all_samples[split_idx:]
    split_dir = output_dir / "splits"
    split_dir.mkdir(exist_ok=True)
    with open(split_dir / "train.jsonl", "w") as f:
        f.write("\n".join(train_split) + "\n")
    with open(split_dir / "val.jsonl", "w") as f:
        f.write("\n".join(val_split) + "\n")
    logging.info(f"  Train: {len(train_split):,} | Val: {len(val_split):,}")
    logging.info(f"  Splits saved to {split_dir}/")


# ---------------------------------------------------------------------------
# Model auto-detection
# ---------------------------------------------------------------------------

async def detect_model(base_url: str, api_key: str) -> str:
    """Auto-detect the model name from the endpoint."""
    # Bedrock doesn't have a /models endpoint — model must be specified via --model
    if _is_bedrock_url(base_url):
        logging.info("Bedrock endpoint detected — model must be specified via --model")
        return ""

    headers = {}
    if api_key:
        headers["Authorization"] = f"Bearer {api_key}"
    try:
        async with aiohttp.ClientSession() as session:
            async with session.get(f"{base_url}/models", headers=headers,
                                   timeout=aiohttp.ClientTimeout(total=10)) as resp:
                if resp.status == 200:
                    data = await resp.json()
                    models = data.get("data", [])
                    if models:
                        model_id = models[0]["id"]
                        logging.info(f"Auto-detected model: {model_id}")
                        return model_id
    except Exception as e:
        logging.warning(f"Could not auto-detect model: {e}")
    return "default"


# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------

def main():
    parser = argparse.ArgumentParser(
        description="SottoASR Synthetic Training Data Generator",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog="""
Examples:
  python generate_synthetic.py --target 100000 --concurrency 8
  python generate_synthetic.py --target 5000 --category self_correction
  python generate_synthetic.py --resume
  python generate_synthetic.py --base-url http://localhost:8000/v1
        """,
    )
    parser.add_argument("--target", type=int, default=DEFAULT_TARGET, help=f"Target valid samples (default: {DEFAULT_TARGET:,})")
    parser.add_argument("--concurrency", type=int, default=DEFAULT_CONCURRENCY, help=f"Concurrent API requests (default: {DEFAULT_CONCURRENCY})")
    parser.add_argument("--category", type=str, default=None, choices=[c["name"] for c in CATEGORIES], help="Generate only one category")
    parser.add_argument("--base-url", type=str, default=os.environ.get("VLLM_BASE_URL", DEFAULT_BASE_URL), help="vLLM API base URL")
    parser.add_argument("--api-key", type=str, default=os.environ.get("VLLM_API_KEY", ""), help="API key (if required)")
    parser.add_argument("--model", type=str, default=os.environ.get("VLLM_MODEL", ""), help="Model name (auto-detect if empty)")
    parser.add_argument("--resume", action="store_true", help="Resume from last checkpoint")
    parser.add_argument("--output-dir", type=Path, default=DEFAULT_OUTPUT_DIR, help=f"Output directory")
    parser.add_argument("--seed", type=int, default=42, help="Random seed (diversity selection only)")
    parser.add_argument("--verbose", action="store_true", help="Debug logging")

    args = parser.parse_args()

    logging.basicConfig(
        level=logging.DEBUG if args.verbose else logging.INFO,
        format="%(asctime)s [%(levelname)s] %(message)s",
        datefmt="%H:%M:%S",
    )

    random.seed(args.seed)

    # Auto-detect model
    model = args.model or asyncio.run(detect_model(args.base_url, args.api_key))

    logging.info("SottoASR Synthetic Data Generator")
    logging.info(f"  Endpoint: {args.base_url}")
    logging.info(f"  Model:    {model}")
    logging.info(f"  Target:   {args.target:,} samples")
    logging.info(f"  Workers:  {args.concurrency}")
    if args.category:
        logging.info(f"  Category: {args.category}")

    try:
        asyncio.run(run_generation(
            base_url=args.base_url,
            model=model,
            api_key=args.api_key,
            target=args.target,
            concurrency=args.concurrency,
            output_dir=args.output_dir,
            category=args.category,
            resume=args.resume,
        ))
    except KeyboardInterrupt:
        logging.info("\nInterrupted by user. Checkpoint was saved — use --resume to continue.")


if __name__ == "__main__":
    main()
