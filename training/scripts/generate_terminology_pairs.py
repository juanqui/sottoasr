#!/usr/bin/env python3
"""
SottoASR Terminology Training Data Generator
=============================================

Generates training pairs for domain-specific term correction.
Takes the terminology database (terms.json) and produces realistic
(misheard_input, corrected_output) pairs in JSONL format.

Supports two modes:
  1. Template mode (default): Fast, deterministic, no API needed.
     Uses sentence templates with term substitution.
  2. LLM mode (--llm): Uses vLLM endpoint for diverse generation.
     Slower but produces more natural, varied sentences.

Usage:
  # Template mode — fast, generates ~5 pairs per term
  python generate_terminology_pairs.py --target 2000

  # LLM mode — diverse, uses vLLM endpoint
  python generate_terminology_pairs.py --target 5000 --llm --concurrency 4

  # Filter by category
  python generate_terminology_pairs.py --target 500 --category ai_company

  # Preview without writing
  python generate_terminology_pairs.py --target 20 --preview

Environment (LLM mode only):
  VLLM_BASE_URL  - vLLM endpoint (default: http://192.168.1.128:8000/v1)
  VLLM_API_KEY   - API key if required
  VLLM_MODEL     - Model name (default: auto-detect)
"""

import argparse
import asyncio
import json
import logging
import os
import random
import re
import sys
import time
from collections import Counter
from pathlib import Path
from typing import Optional

try:
    import aiohttp
except ImportError:
    aiohttp = None  # Will fail at runtime if --llm/--bedrock used

logging.basicConfig(level=logging.INFO, format="%(asctime)s %(levelname)s %(message)s")
log = logging.getLogger(__name__)

# ---------------------------------------------------------------------------
# Paths
# ---------------------------------------------------------------------------

SCRIPT_DIR = Path(__file__).parent
TERMS_FILE = SCRIPT_DIR.parent / "data" / "terminology" / "terms.json"
OUTPUT_DIR = SCRIPT_DIR.parent / "data" / "terminology"
OUTPUT_FILE = OUTPUT_DIR / "terminology_pairs.jsonl"

# ---------------------------------------------------------------------------
# LLM config
# ---------------------------------------------------------------------------

DEFAULT_BASE_URL = "http://192.168.1.128:8000/v1"
TEMPERATURE = 0.9
TOP_P = 0.95
MAX_TOKENS = 2048
BATCH_SIZE = 10

# Bedrock defaults
BEDROCK_DEFAULT_URL = "https://bedrock-runtime.us-east-1.amazonaws.com"
BEDROCK_DEFAULT_MODEL = "us.anthropic.claude-haiku-4-5-20251001-v1:0"

# ---------------------------------------------------------------------------
# Sentence templates — organized by domain context
# ---------------------------------------------------------------------------

# Each template has:
#   - A sentence with {term} placeholder (for the CORRECT term)
#   - Domain tags for weighted selection

TEMPLATES = {
    "ai_company": [
        "we should look into what {term} is doing with their latest release",
        "did you see the blog post from {term} about their new model",
        "{term} just announced a partnership with microsoft",
        "i think {term} has the best approach to ai safety right now",
        "we need to evaluate {term} for our enterprise ai stack",
        "the pricing from {term} is actually pretty competitive",
        "i was reading the {term} documentation yesterday",
        "{term} published a research paper on scaling laws",
        "our team is switching from openai to {term} for the api",
        "the {term} playground is great for prototyping",
        "{term} raised another round of funding last week",
        "have you tried the {term} api for this use case",
    ],
    "ai_model": [
        "we should try running this through {term}",
        "{term} handles this task much better than the previous version",
        "the benchmark results for {term} look really promising",
        "i tested our prompts on {term} and the quality improved",
        "can you compare the output from {term} versus the other model",
        "we are going to use {term} for the production deployment",
        "{term} has a much larger context window than we expected",
        "the latency on {term} is actually reasonable for our use case",
        "i fine tuned a version of {term} on our dataset",
        "the {term} model card shows impressive safety benchmarks",
    ],
    "ai_coding_tool": [
        "i switched to {term} last week and my productivity doubled",
        "{term} handles multi file refactors really well",
        "the auto completion in {term} is better than what i had before",
        "have you tried using {term} for your typescript projects",
        "we should standardize the team on {term} for pair programming",
        "{term} just shipped a new agent mode that looks interesting",
        "i wrote the entire module using {term} in about an hour",
        "the code suggestions from {term} are surprisingly accurate",
    ],
    "ai_framework": [
        "we are building the rag pipeline with {term}",
        "{term} makes it easy to chain multiple llm calls together",
        "i set up a local instance using {term} on my macbook",
        "the latest version of {term} has much better memory management",
        "{term} supports streaming responses out of the box",
        "we should benchmark {term} against the alternatives",
        "the documentation for {term} is actually really thorough",
        "i deployed the model using {term} and got great throughput",
        "our agent workflow is built on top of {term}",
        "{term} handles concurrent requests much better now",
    ],
    "ai_concept": [
        "we implemented {term} to improve the model output quality",
        "the paper describes a novel approach to {term}",
        "have you looked into using {term} for this fine tuning job",
        "{term} reduced our training cost by about sixty percent",
        "the {term} results were significantly better than baseline",
        "i think {term} is the right technique for our use case",
        "we need to evaluate whether {term} makes sense here",
        "the team is implementing {term} in the next sprint",
    ],
    "ai_protocol": [
        "we should build an {term} server for our internal tools",
        "{term} is becoming the standard for llm tool integration",
        "the {term} specification was updated last week",
        "our agent communicates with external services via {term}",
        "have you set up the {term} connection to the database yet",
        "the {term} protocol handles authentication automatically",
    ],
    "vector_db": [
        "we are using {term} for our semantic search backend",
        "{term} handles our embedding storage really well",
        "the query latency on {term} is under five milliseconds",
        "we migrated from postgres to {term} for the vector search",
        "i set up a {term} cluster for the rag pipeline",
        "{term} supports filtering and hybrid search out of the box",
    ],
    "search_tool": [
        "we integrated {term} for the search functionality",
        "{term} returns much more relevant results than our previous search",
        "the {term} api makes it easy to add search to any app",
        "our search is powered by {term} with custom ranking",
    ],
    "cloud_platform": [
        "we deployed the app to {term} and it was surprisingly easy",
        "{term} handles our auto scaling without any configuration",
        "the {term} free tier is generous enough for our mvp",
        "we are migrating from heroku to {term} next month",
        "i set up the database on {term} with automatic backups",
        "{term} supports edge functions which we need for low latency",
        "the {term} dashboard makes it easy to monitor deployments",
        "our infrastructure runs on {term} in three regions",
    ],
    "web_framework": [
        "we are building the frontend with {term}",
        "{term} has better performance characteristics than react for this",
        "i migrated the project from webpack to {term} and builds are faster",
        "the {term} docs have a great getting started tutorial",
        "we should evaluate {term} for the new project",
        "{term} handles server side rendering out of the box",
        "the developer experience with {term} is really nice",
        "our component library is built on {term}",
        "i just upgraded to the latest version of {term}",
    ],
    "data_tool": [
        "we switched to {term} for our database layer",
        "{term} gives us full type safety for all our queries",
        "the migration from sequelize to {term} was straightforward",
        "i really like how {term} handles joins and relations",
        "{term} generates the sql for us with proper typing",
        "we are using {term} for schema validation on the api",
    ],
    "devops_tool": [
        "we set up {term} for monitoring the production environment",
        "{term} alerted us to the memory leak before it caused an outage",
        "the {term} dashboard shows all our service dependencies",
        "we are implementing distributed tracing with {term}",
        "i configured {term} to handle our deployment pipeline",
        "{term} integrates well with our existing kubernetes setup",
    ],
    "auth_tool": [
        "we implemented authentication using {term}",
        "{term} handles the social login flow for us",
        "the {term} integration took about two hours to set up",
        "we need to add {term} support for enterprise customers",
        "our sso implementation uses {term} under the hood",
        "{term} manages all our user sessions and tokens",
    ],
    "testing_tool": [
        "we write our unit tests with {term}",
        "{term} catches browser compatibility issues in ci",
        "the {term} test runner is much faster than what we used before",
        "i set up {term} for our end to end test suite",
    ],
    "protocol": [
        "we use {term} for the real time communication layer",
        "the {term} implementation handles bidirectional streaming",
        "we switched from rest to {term} for the internal apis",
        "our {term} server handles thousands of concurrent connections",
    ],
    "language": [
        "we rewrote the performance critical parts in {term}",
        "{term} gives us better memory safety guarantees",
        "the team has been learning {term} for the backend rewrite",
        "i really like the concurrency model in {term}",
    ],
    "infrastructure": [
        "we deploy to a {term} cluster in production",
        "the {term} configuration handles load balancing automatically",
        "i wrote the infrastructure as code using {term}",
        "our {term} setup handles about ten thousand requests per second",
    ],
    "database": [
        "we store all the user data in {term}",
        "{term} handles our write heavy workload really well",
        "the {term} query optimizer improved our p99 latency",
        "we added a read replica in {term} for the analytics queries",
    ],
    "hardware": [
        "we need {term} to run the model at acceptable speed",
        "the {term} has enough memory for our largest model",
        "training on {term} reduced our iteration time significantly",
    ],
}

# Default template for categories without specific ones
DEFAULT_TEMPLATES = [
    "we are using {term} for this project",
    "have you tried {term} for this use case",
    "the {term} integration works really well",
    "{term} is what the team recommends",
    "i set up {term} yesterday and it works great",
    "we should evaluate {term} as an alternative",
    "the {term} documentation covers this scenario",
    "our stack includes {term} for this layer",
]

# Difficulty-specific disfluency injectors
FILLERS = ["uh", "um", "uhm", "er", "ah"]
CRUTCH_WORDS = ["basically", "you know", "like", "so", "i mean"]


def load_terms(path: Path, category_filter: Optional[str] = None) -> list[dict]:
    """Load terms from the JSON database, optionally filtering by category."""
    with open(path) as f:
        data = json.load(f)
    terms = data["terms"]
    if category_filter:
        terms = [t for t in terms if t["category"] == category_filter]
    return terms


def apply_asr_style(text: str) -> str:
    """Convert clean text to ASR-style output: lowercase, no punctuation, split contractions."""
    text = text.lower()
    # Remove punctuation
    text = re.sub(r"[.,!?;:\"'\-—/()[\]{}]", "", text)
    # Split common contractions
    contractions = {
        "don't": "dont",
        "doesn't": "doesnt",
        "didn't": "didnt",
        "can't": "cant",
        "won't": "wont",
        "wouldn't": "wouldnt",
        "shouldn't": "shouldnt",
        "couldn't": "couldnt",
        "isn't": "isnt",
        "aren't": "arent",
        "wasn't": "wasnt",
        "weren't": "werent",
        "haven't": "havent",
        "hasn't": "hasnt",
        "hadn't": "hadnt",
        "i'm": "im",
        "i've": "ive",
        "i'll": "ill",
        "i'd": "id",
        "we're": "were",
        "we've": "weve",
        "we'll": "well",
        "we'd": "wed",
        "they're": "theyre",
        "they've": "theyve",
        "they'll": "theyll",
        "they'd": "theyd",
        "you're": "youre",
        "you've": "youve",
        "you'll": "youll",
        "you'd": "youd",
        "it's": "its",
        "that's": "thats",
        "there's": "theres",
        "here's": "heres",
        "what's": "whats",
        "who's": "whos",
        "let's": "lets",
    }
    for contraction, replacement in contractions.items():
        text = text.replace(contraction, replacement)
    # Normalize whitespace
    text = re.sub(r"\s+", " ", text).strip()
    return text


def inject_disfluency(text: str, difficulty: str) -> str:
    """Inject fillers/crutch words based on difficulty level."""
    words = text.split()
    if len(words) < 3:
        return text

    if difficulty == "easy":
        # Add 0-1 fillers
        if random.random() < 0.5:
            pos = random.randint(1, max(1, len(words) - 1))
            words.insert(pos, random.choice(FILLERS))
    elif difficulty == "medium":
        # Add 1-2 fillers
        num_fillers = random.randint(1, 2)
        for _ in range(num_fillers):
            pos = random.randint(1, max(1, len(words) - 1))
            filler = random.choice(FILLERS + CRUTCH_WORDS)
            words.insert(pos, filler)
    elif difficulty == "hard":
        # Add 2-4 fillers/crutch words
        num_fillers = random.randint(2, 4)
        for _ in range(num_fillers):
            pos = random.randint(1, max(1, len(words) - 1))
            filler = random.choice(FILLERS + CRUTCH_WORDS)
            words.insert(pos, filler)

    return " ".join(words)


def make_clean_sentence(template: str, term: str) -> str:
    """Fill a template with the correct term and apply proper formatting."""
    sentence = template.replace("{term}", term)
    # Capitalize first letter, but preserve if {term} was at position 0
    if not sentence[0].isupper():
        sentence = sentence[0].upper() + sentence[1:]
    # Use question mark for questions, period otherwise
    if not sentence[-1] in ".!?":
        if sentence.lower().startswith(("have you", "can you", "did you", "do you", "is the", "are the", "should we", "could you", "what", "how", "why", "where", "when")):
            sentence += "?"
        else:
            sentence += "."
    return sentence


def generate_template_pair(term_entry: dict) -> dict:
    """Generate a single training pair using templates."""
    term = term_entry["term"]
    category = term_entry["category"]
    confusions = term_entry["confusions"]

    # Pick a confusion (skip identity confusions that are just lowercase)
    real_confusions = [c for c in confusions if c.lower().replace(" ", "") != term.lower().replace(" ", "")]
    if not real_confusions:
        # If all confusions are trivial, use the first one
        confusion = confusions[0]
    else:
        confusion = random.choice(real_confusions)

    # Pick a template
    templates = TEMPLATES.get(category, DEFAULT_TEMPLATES)
    template = random.choice(templates)

    # Generate clean output
    clean = make_clean_sentence(template, term)

    # Generate raw input: apply ASR style first WITHOUT the term,
    # then insert the confusion term to avoid fillers splitting it
    confusion_lower = confusion.lower()
    placeholder = "TERM_PLACEHOLDER_XYZ"
    raw_template = template.replace("{term}", placeholder)
    raw = apply_asr_style(raw_template)

    # Difficulty weighting
    difficulty = random.choices(
        ["easy", "medium", "hard"],
        weights=[0.30, 0.45, 0.25],
        k=1,
    )[0]

    # Inject disfluencies BEFORE inserting the term (so fillers don't split it)
    raw = inject_disfluency(raw, difficulty)

    # Now insert the confusion term
    raw = raw.replace(placeholder.lower(), confusion_lower)

    return {
        "input": raw,
        "output": clean,
        "category": "misheard_words",
        "domain": _category_to_domain(category),
        "difficulty": difficulty,
        "misheard_term": confusion,
        "correct_term": term,
        "term_category": category,
    }


def _category_to_domain(category: str) -> str:
    """Map term category to training domain."""
    domain_map = {
        "ai_company": "software_engineering",
        "ai_model": "software_engineering",
        "ai_coding_tool": "software_engineering",
        "ai_framework": "software_engineering",
        "ai_concept": "software_engineering",
        "ai_protocol": "software_engineering",
        "vector_db": "software_engineering",
        "search_tool": "software_engineering",
        "cloud_platform": "software_engineering",
        "web_framework": "software_engineering",
        "data_tool": "software_engineering",
        "devops_tool": "software_engineering",
        "auth_tool": "software_engineering",
        "testing_tool": "software_engineering",
        "protocol": "software_engineering",
        "language": "software_engineering",
        "infrastructure": "software_engineering",
        "database": "software_engineering",
        "hardware": "technical_other",
        "rust_lib": "software_engineering",
        "apple_tech": "technical_other",
        "crypto": "finance",
        "fintech_tool": "finance",
        "productivity_tool": "general_business",
        "mobile_tool": "software_engineering",
        "no_code": "general_business",
        "automation": "general_business",
        "api_service": "software_engineering",
        "dev_platform": "software_engineering",
    }
    return domain_map.get(category, "software_engineering")


def generate_template_pairs(terms: list[dict], target: int) -> list[dict]:
    """Generate training pairs using template substitution."""
    pairs = []
    seen_raw = set()

    # Calculate how many pairs per term (approximately)
    pairs_per_term = max(1, target // len(terms))
    remaining = target

    # Shuffle terms for variety
    shuffled_terms = terms.copy()
    random.shuffle(shuffled_terms)

    rounds = 0
    while remaining > 0 and rounds < 50:
        rounds += 1
        for term_entry in shuffled_terms:
            if remaining <= 0:
                break

            pair = generate_template_pair(term_entry)

            # Dedup by raw text
            if pair["input"] in seen_raw:
                continue

            seen_raw.add(pair["input"])
            pairs.append(pair)
            remaining -= 1

        random.shuffle(shuffled_terms)

    return pairs


# ---------------------------------------------------------------------------
# LLM-based generation
# ---------------------------------------------------------------------------

LLM_SYSTEM_PROMPT = """You are a training data generator for an ASR transcript cleanup model. You generate realistic (raw_transcript, cleaned_transcript) pairs that teach the model to correct misheard domain-specific terms.

CRITICAL RULES for the "raw" field:
- ALL LOWERCASE, no capital letters
- NO PUNCTUATION: no periods, commas, question marks, exclamation points, colons, semicolons, hyphens, or apostrophes
- Contractions are split: "dont" not "don't", "im" not "I'm"
- Compound words are separated: "web socket" not "websocket"
- The misheard term MUST appear as the phonetic confusion, not the correct spelling

CRITICAL RULES for the "clean" field:
- Proper punctuation, capitalization, and formatting
- The correct term MUST appear with proper spelling and capitalization
- NEVER paraphrase or add content not in "raw"

Output ONLY a valid JSON array, no markdown fencing, no commentary."""


def build_llm_prompt(term_batch: list[dict], batch_size: int = BATCH_SIZE) -> str:
    """Build a prompt for LLM-based generation of terminology pairs."""
    term_lines = []
    for t in term_batch:
        confusions_str = ", ".join(f'"{c}"' for c in t["confusions"][:3])
        term_lines.append(
            f'  - Correct: "{t["term"]}" | Misheard as: {confusions_str} | Context: {t["context"]}'
        )

    terms_block = "\n".join(term_lines)

    return f"""Generate exactly {batch_size} transcript cleanup training pairs where ASR has misheard domain-specific terms.

TERMS TO USE (pick from these, vary which confusion you use):
{terms_block}

Each pair should:
1. Use one of the terms above, replacing it with one of its phonetic confusions in the "raw" field
2. Use the correct term in the "clean" field
3. Be a natural sentence a software engineer or knowledge worker would dictate
4. Optionally include 0-2 fillers (uh, um) for realism
5. Vary sentence length (5-40 words) and structure

Output a JSON array of objects with "raw", "clean", "misheard_term", and "correct_term" fields:
[{{"raw": "we should use the exah search api for this", "clean": "We should use the Exa search API for this.", "misheard_term": "exah", "correct_term": "Exa"}}, ...]

Generate {batch_size} DIVERSE examples. Vary sentence structure and context."""


def _is_bedrock_url(base_url: str) -> bool:
    """Check if this is a Bedrock Converse API endpoint."""
    return "bedrock-runtime" in base_url and "amazonaws.com" in base_url


async def _call_bedrock(session, base_url: str, model: str,
                        api_key: str, system: str, user: str) -> Optional[str]:
    """Call AWS Bedrock Converse API."""
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
                    log.warning("Bedrock rate limited, waiting %.1fs...", wait)
                    await asyncio.sleep(wait)
                elif resp.status >= 500:
                    wait = 2 ** attempt + random.random()
                    log.warning("Bedrock server error %d, retrying...", resp.status)
                    await asyncio.sleep(wait)
                else:
                    body = await resp.text()
                    log.error("Bedrock error %d: %s", resp.status, body[:200])
                    return None
        except asyncio.TimeoutError:
            log.warning("Bedrock timeout on attempt %d/3", attempt + 1)
            await asyncio.sleep(2 ** attempt)
        except Exception as e:
            log.error("Bedrock request error: %s", e)
            await asyncio.sleep(2 ** attempt)
    return None


async def _call_openai(session, base_url: str, model: str,
                       api_key: str, system: str, user: str) -> Optional[str]:
    """Call OpenAI-compatible API (vLLM, etc.)."""
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
    }

    try:
        async with session.post(
            f"{base_url}/chat/completions",
            json=payload,
            headers=headers,
            timeout=aiohttp.ClientTimeout(total=120),
        ) as resp:
            if resp.status != 200:
                log.warning("API error %d: %s", resp.status, (await resp.text())[:200])
                return None
            data = await resp.json()
            return data["choices"][0]["message"]["content"]
    except Exception as e:
        log.warning("Request failed: %s", e)
        return None


async def _call_llm(session, base_url: str, model: str,
                    api_key: str, system: str, user: str) -> Optional[str]:
    """Unified LLM call — routes to Bedrock Converse or OpenAI-compatible."""
    if _is_bedrock_url(base_url):
        return await _call_bedrock(session, base_url, model, api_key, system, user)
    else:
        return await _call_openai(session, base_url, model, api_key, system, user)


def _parse_json_response(text: str) -> list[dict]:
    """Extract a JSON array from the LLM response."""
    if not text:
        return []
    # Strip markdown fences if present
    text = re.sub(r"```(?:json)?\s*", "", text)
    text = text.strip().rstrip("`")
    try:
        return json.loads(text)
    except json.JSONDecodeError:
        match = re.search(r"\[.*\]", text, re.DOTALL)
        if match:
            try:
                return json.loads(match.group())
            except json.JSONDecodeError:
                pass
        log.warning("Failed to parse JSON from response")
        return []


async def generate_llm_batch(
    session,
    base_url: str,
    model: str,
    api_key: str,
    term_batch: list[dict],
    batch_size: int = BATCH_SIZE,
) -> list[dict]:
    """Generate a batch of pairs using the LLM endpoint."""
    prompt = build_llm_prompt(term_batch, batch_size)
    content = await _call_llm(session, base_url, model, api_key, LLM_SYSTEM_PROMPT, prompt)
    samples = _parse_json_response(content)

    # Validate and normalize
    valid = []
    for s in samples:
        if not isinstance(s, dict):
            continue
        raw = s.get("raw", "").strip()
        clean = s.get("clean", "").strip()
        if not raw or not clean:
            continue
        if len(raw.split()) < 2 or len(clean.split()) < 2:
            continue

        # Estimate difficulty from length
        word_count = len(raw.split())
        if word_count <= 15:
            difficulty = "easy"
        elif word_count <= 40:
            difficulty = "medium"
        else:
            difficulty = "hard"

        valid.append({
            "input": raw,
            "output": clean,
            "category": "misheard_words",
            "domain": "software_engineering",
            "difficulty": difficulty,
            "misheard_term": s.get("misheard_term", ""),
            "correct_term": s.get("correct_term", ""),
            "term_category": "llm_generated",
        })

    return valid


def _load_env_file():
    """Load .env file from project root if it exists."""
    env_path = SCRIPT_DIR.parent.parent / ".env"
    if env_path.exists():
        with open(env_path) as f:
            for line in f:
                line = line.strip()
                if line and not line.startswith("#") and "=" in line:
                    key, _, value = line.partition("=")
                    os.environ.setdefault(key.strip(), value.strip())


async def generate_llm_pairs(
    terms: list[dict],
    target: int,
    concurrency: int = 4,
    model_override: str = "",
    bedrock: bool = False,
) -> list[dict]:
    """Generate pairs using LLM endpoint with bounded concurrency."""
    try:
        import aiohttp
    except ImportError:
        log.error("aiohttp required for LLM mode. Install: pip install aiohttp")
        sys.exit(1)

    # Load .env for API keys
    _load_env_file()

    if bedrock:
        base_url = os.environ.get("VLLM_BASE_URL", BEDROCK_DEFAULT_URL)
        api_key = os.environ.get("VLLM_API_KEY",
                                 os.environ.get("AWS_BEARER_TOKEN_BEDROCK", ""))
        model = model_override or os.environ.get("VLLM_MODEL", BEDROCK_DEFAULT_MODEL)
        log.info("Using Bedrock: %s model=%s", base_url, model)
    else:
        base_url = os.environ.get("VLLM_BASE_URL", DEFAULT_BASE_URL)
        api_key = os.environ.get("VLLM_API_KEY", "")
        model = model_override or os.environ.get("VLLM_MODEL", "")

        # Auto-detect model if not specified
        if not model:
            import urllib.request
            try:
                req = urllib.request.Request(f"{base_url}/models")
                with urllib.request.urlopen(req, timeout=10) as resp:
                    models_data = json.loads(resp.read())
                    model = models_data["data"][0]["id"]
                    log.info("Auto-detected model: %s", model)
            except Exception:
                log.error("Cannot auto-detect model. Set VLLM_MODEL or use --bedrock.")
                sys.exit(1)

    pairs = []
    seen_raw = set()
    sem = asyncio.Semaphore(concurrency)
    completed_batches = 0

    async with aiohttp.ClientSession() as session:
        async def worker(term_batch):
            nonlocal completed_batches
            async with sem:
                results = await generate_llm_batch(
                    session, base_url, model, api_key, term_batch)
                completed_batches += 1
                return results

        # Create batches of 3-5 terms each
        random.shuffle(terms)
        batches_needed = (target // BATCH_SIZE) + 1
        tasks = []

        for i in range(batches_needed):
            batch_terms = random.sample(terms, min(random.randint(3, 5), len(terms)))
            tasks.append(worker(batch_terms))

        log.info("Launching %d LLM batches with concurrency=%d", len(tasks), concurrency)

        for coro in asyncio.as_completed(tasks):
            results = await coro
            for pair in results:
                if pair["input"] not in seen_raw:
                    seen_raw.add(pair["input"])
                    pairs.append(pair)

            if len(pairs) >= target:
                break

            if completed_batches % 10 == 0:
                log.info("Progress: %d / %d pairs (%d batches done)",
                         len(pairs), target, completed_batches)

    return pairs[:target]


# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------

def main():
    parser = argparse.ArgumentParser(description="Generate terminology training pairs")
    parser.add_argument("--target", type=int, default=2000, help="Number of pairs to generate")
    parser.add_argument("--category", type=str, help="Filter to specific term category")
    parser.add_argument("--output", type=str, help="Output file path (default: terminology_pairs.jsonl)")
    parser.add_argument("--llm", action="store_true", help="Use LLM endpoint instead of templates")
    parser.add_argument("--bedrock", action="store_true", help="Use AWS Bedrock (Haiku 4.5 global inference)")
    parser.add_argument("--model", type=str, default="", help="Override model ID")
    parser.add_argument("--concurrency", type=int, default=4, help="LLM mode concurrency")
    parser.add_argument("--preview", action="store_true", help="Print pairs instead of writing")
    parser.add_argument("--seed", type=int, default=42, help="Random seed")
    parser.add_argument("--append", action="store_true", help="Append to existing output file")
    args = parser.parse_args()

    random.seed(args.seed)
    output_path = Path(args.output) if args.output else OUTPUT_FILE

    # Load terms
    if not TERMS_FILE.exists():
        log.error("Terms file not found: %s", TERMS_FILE)
        sys.exit(1)

    terms = load_terms(TERMS_FILE, args.category)
    log.info("Loaded %d terms%s", len(terms), f" (category={args.category})" if args.category else "")

    if not terms:
        log.error("No terms found for category: %s", args.category)
        sys.exit(1)

    # Generate pairs
    start = time.time()
    if args.bedrock:
        args.llm = True  # --bedrock implies --llm
    if args.llm:
        pairs = asyncio.run(generate_llm_pairs(
            terms, args.target, args.concurrency,
            model_override=args.model, bedrock=args.bedrock))
    else:
        pairs = generate_template_pairs(terms, args.target)
    elapsed = time.time() - start

    log.info("Generated %d pairs in %.1fs", len(pairs), elapsed)

    # Stats
    cat_counts = Counter(p["term_category"] for p in pairs)
    diff_counts = Counter(p["difficulty"] for p in pairs)
    log.info("By term category: %s", dict(sorted(cat_counts.items(), key=lambda x: -x[1])))
    log.info("By difficulty: %s", dict(diff_counts))

    if args.preview:
        for p in pairs[:30]:
            print(f"\n  raw:   {p['input']}")
            print(f"  clean: {p['output']}")
            print(f"  [{p['misheard_term']} → {p['correct_term']}]")
        return

    # Write output
    output_path.parent.mkdir(parents=True, exist_ok=True)
    mode = "a" if args.append else "w"
    with open(output_path, mode) as f:
        for p in pairs:
            # Write in the same format as existing misheard_words.jsonl
            record = {
                "input": p["input"],
                "output": p["output"],
                "category": p["category"],
                "domain": p["domain"],
                "difficulty": p["difficulty"],
                "misheard_term": p["misheard_term"],
                "correct_term": p["correct_term"],
            }
            f.write(json.dumps(record, ensure_ascii=False) + "\n")

    log.info("Wrote %d pairs to %s", len(pairs), output_path)


if __name__ == "__main__":
    main()
