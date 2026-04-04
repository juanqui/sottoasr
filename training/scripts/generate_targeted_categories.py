#!/usr/bin/env python3
"""
Generate targeted training data for underrepresented categories using Bedrock Haiku.

Categories and targets (2x):
1. Long-form dictation (50-300 words): 6,000
2. Run-on sentence splitting: 2,000
3. Casual/personal domain: 6,000
4. Multi-speaker / meetings: 1,000
5. ASR engine artifacts: 1,000
6. Natural number formatting: 1,000

Total: ~17,000 entries
"""
import asyncio
import aiohttp
import json
import random
import time
import os
import sys
from pathlib import Path
from collections import Counter

# ── Config ──
BEDROCK_URL = "https://bedrock-runtime.us-east-1.amazonaws.com/model/global.anthropic.claude-haiku-4-5-20251001-v1:0/converse"
API_KEY = os.environ.get("AWS_BEARER_TOKEN_BEDROCK", "")
CONCURRENCY = 32
OUTPUT_DIR = Path("training/data/generated/splits_combined")
OUTPUT_DIR.mkdir(parents=True, exist_ok=True)

# ── Category definitions ──
CATEGORIES = {
    "long_form": {
        "target": 6000,
        "system": """You generate training data for an ASR transcript cleanup model.
Generate a SINGLE training pair: a raw ASR transcript (input) and its cleaned version (output).

The INPUT must be a LONG stream-of-consciousness spoken paragraph (80-200 words). It should:
- Have NO punctuation or capitalization (raw ASR output)
- Include natural fillers: uh, um, like, you know, basically, so, okay
- Include self-corrections, false starts, and topic transitions
- Cover professional OR personal topics
- Feel like a real person speaking continuously for 30-60 seconds

The OUTPUT must be the properly cleaned, punctuated, formatted version.

Respond with ONLY valid JSON: {"input": "...", "output": "..."}""",
    },
    "run_on": {
        "target": 2000,
        "system": """You generate training data for an ASR transcript cleanup model.
Generate a SINGLE training pair where the input is a run-on stream of multiple sentences with NO punctuation.

The INPUT must be 3-6 sentences run together as one block with no periods, no commas, no capitalization. Include light fillers.
The OUTPUT must split these into properly punctuated separate sentences.

Example:
input: "the server is down we need to fix it before the client notices also the backup failed last night"
output: "The server is down. We need to fix it before the client notices. Also, the backup failed last night."

Respond with ONLY valid JSON: {"input": "...", "output": "..."}""",
    },
    "casual_personal": {
        "target": 6000,
        "system": """You generate training data for an ASR transcript cleanup model.
Generate a SINGLE training pair of CASUAL/PERSONAL speech (not work or tech).

Topics: grocery lists, family plans, texting friends, personal reminders, social events, home tasks,
hobbies, travel plans, health/fitness, cooking, pets, weather, sports, entertainment, relationships.

The INPUT must be informal spoken language with fillers (uh, um, like, so, basically, you know).
The OUTPUT must be clean but maintain casual tone — don't over-formalize.

Example:
input: "uh remind me to pick up milk and uh eggs on the way home oh and also dog food"
output: "Remind me to pick up milk, eggs, and dog food on the way home."

Respond with ONLY valid JSON: {"input": "...", "output": "..."}""",
    },
    "multi_speaker": {
        "target": 1000,
        "system": """You generate training data for an ASR transcript cleanup model.
Generate a SINGLE training pair of a meeting/conversation transcript with multiple speakers.

The INPUT should be a raw transcript of 2-3 speakers with:
- Speaker attributions (like "john said" or names mentioned)
- Cross-talk, interruptions, and backtracking
- Fillers from multiple speakers
- No punctuation, lowercase

The OUTPUT should be a clean summary or properly formatted multi-speaker transcript.

Respond with ONLY valid JSON: {"input": "...", "output": "..."}""",
    },
    "asr_artifacts": {
        "target": 1000,
        "system": """You generate training data for an ASR transcript cleanup model.
Generate a SINGLE training pair where the input contains REALISTIC ASR engine artifacts:

Pick ONE or more of these real ASR problems:
- Hallucinated phrases at silence: "thank you for watching" "please subscribe" "music playing"
- Repeated segments from chunk boundaries: "we need to we need to fix the build"
- Word boundary errors: "alot" "incase" "atleast" "eachother" "thankyou"
- Homophone confusion: "their" for "they're", "your" for "you're", "its" for "it's", "weather" for "whether"
- Missing/inserted words: "the server down" (missing "is"), "we we need" (inserted repeat)
- Systematic misrecognition of names/brands: "siri" for "series", proper nouns lowercased

The OUTPUT should fix ALL these artifacts into correct text.

Respond with ONLY valid JSON: {"input": "...", "output": "..."}""",
    },
    "number_formatting": {
        "target": 1000,
        "system": """You generate training data for an ASR transcript cleanup model.
Generate a SINGLE training pair focused on NUMBER FORMATTING in natural speech.

The INPUT should contain numbers spoken as words (ASR typically outputs words not digits).
The OUTPUT should format them appropriately based on context:
- Times: "two thirty pm" → "2:30 PM"
- Dates: "march fifteenth" → "March 15th"
- Money: "fifty dollars" → "$50"
- Percentages: "twenty five percent" → "25%"
- Phone: "five five five one two three four" → "555-1234"
- Addresses: "one two three main street" → "123 Main Street"
- Measurements: "six feet two inches" → "6'2""
- Room/flight numbers: "room four oh three" → "Room 403"
- Scores/stats: "three to two" → "3-2"
- Quantities over 10 → digits: "about twenty five people" → "about 25 people"

Include fillers in the input. Respond with ONLY valid JSON: {"input": "...", "output": "..."}""",
    },
}


async def call_bedrock(session, system_prompt, semaphore):
    """Call Bedrock Converse API."""
    async with semaphore:
        headers = {
            "Authorization": f"Bearer {API_KEY}",
            "Content-Type": "application/json",
        }
        body = {
            "system": [{"text": system_prompt}],
            "messages": [
                {"role": "user", "content": [{"text": "Generate one training pair now."}]}
            ],
            "inferenceConfig": {
                "maxTokens": 2048,
                "temperature": 0.9,
            },
        }

        try:
            async with session.post(BEDROCK_URL, json=body, headers=headers, timeout=aiohttp.ClientTimeout(total=30)) as resp:
                if resp.status != 200:
                    text = await resp.text()
                    if resp.status == 429:
                        await asyncio.sleep(2)
                    return None
                result = await resp.json()
                content = result.get("output", {}).get("message", {}).get("content", [])
                if content:
                    text = content[0].get("text", "")
                    # Extract JSON from response
                    text = text.strip()
                    if text.startswith("```"):
                        text = text.split("\n", 1)[1] if "\n" in text else text[3:]
                        text = text.rsplit("```", 1)[0]
                    return json.loads(text)
        except (json.JSONDecodeError, asyncio.TimeoutError, aiohttp.ClientError, KeyError):
            return None
    return None


def validate_pair(pair, category):
    """Basic validation of a generated pair."""
    if not pair or not isinstance(pair, dict):
        return False
    inp = pair.get("input", "")
    out = pair.get("output", "")
    if not inp or not out:
        return False
    if len(inp) < 10 or len(out) < 5:
        return False
    # Output shouldn't be much longer than input
    if len(out) > len(inp) * 2 + 50:
        return False
    # Long-form must actually be long
    if category == "long_form" and len(inp.split()) < 40:
        return False
    return True


async def generate_category(category_name, config):
    """Generate all entries for one category."""
    target = config["target"]
    system = config["system"]
    output_file = OUTPUT_DIR / f"{category_name}.jsonl"

    # Resume from existing file
    existing = []
    if output_file.exists():
        with open(output_file) as f:
            for line in f:
                if line.strip():
                    existing.append(json.loads(line))

    valid_count = len(existing)
    if valid_count >= target:
        print(f"  [{category_name}] Already have {valid_count}/{target} — skipping")
        return valid_count

    print(f"  [{category_name}] Starting: {valid_count}/{target} existing")

    semaphore = asyncio.Semaphore(CONCURRENCY)
    rejected = 0
    stall_time = time.time()

    async with aiohttp.ClientSession() as session:
        while valid_count < target:
            # Launch batch of concurrent requests
            batch_size = min(CONCURRENCY * 2, target - valid_count + 10)
            tasks = [call_bedrock(session, system, semaphore) for _ in range(batch_size)]
            results = await asyncio.gather(*tasks)

            batch_valid = 0
            with open(output_file, "a") as f:
                for pair in results:
                    if validate_pair(pair, category_name):
                        f.write(json.dumps(pair) + "\n")
                        valid_count += 1
                        batch_valid += 1
                        stall_time = time.time()
                    else:
                        rejected += 1

            elapsed = time.time() - stall_time
            rate = valid_count / max(1, time.time() - stall_time) if batch_valid == 0 else batch_valid
            print(f"  [{category_name}] {valid_count}/{target} ({100*valid_count/target:.0f}%) | batch: +{batch_valid} | rejected: {rejected}")

            # Stall detection
            if elapsed > 120 and batch_valid == 0:
                print(f"  [{category_name}] STALL — no valid samples in 120s. Stopping.")
                break

            # Rate limiting
            if batch_valid == 0:
                await asyncio.sleep(3)

    return valid_count


async def main():
    print(f"Bedrock Targeted Data Generator")
    print(f"  Endpoint: {BEDROCK_URL}")
    print(f"  Concurrency: {CONCURRENCY}")
    print(f"  Output: {OUTPUT_DIR}")
    print()

    total = 0
    for name, config in CATEGORIES.items():
        print(f"\n{'='*50}")
        print(f"Category: {name} (target: {config['target']})")
        print(f"{'='*50}")
        count = await generate_category(name, config)
        total += count
        print(f"  [{name}] Done: {count}")

    print(f"\n{'='*50}")
    print(f"TOTAL GENERATED: {total}")
    print(f"{'='*50}")


if __name__ == "__main__":
    if not API_KEY:
        print("ERROR: AWS_BEARER_TOKEN_BEDROCK not set in environment")
        sys.exit(1)
    asyncio.run(main())
