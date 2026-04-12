#!/usr/bin/env python3
"""Upload paragraph_formatting samples to juanquivilla/sotto-transcript-cleanup.

Loads the existing HF dataset, appends the new paragraph rows into TRAIN split,
and pushes to the hub with a descriptive commit message. Preserves the validation
split untouched.

Usage:
    python upload_paragraph_samples.py \\
        --jsonl ../data/generated_bedrock_paragraphs/train.jsonl \\
        --commit-message "v23+paragraphs: +4000 paragraph_formatting rows via Bedrock/Haiku 4.5"
"""

from __future__ import annotations

import argparse
import json
import os
import sys
from pathlib import Path


DEFAULT_JSONL = Path("/Users/juanqui/Git/sotto/training/data/generated_bedrock_paragraphs/train.jsonl")
DEFAULT_MSG = "v23+paragraphs: +4000 paragraph_formatting rows via Bedrock/Haiku 4.5"
DATASET_ID = "juanquivilla/sotto-transcript-cleanup"


def load_hf_token() -> str:
    """Read HF_TOKEN from .env if not already in environment."""
    if os.environ.get("HF_TOKEN"):
        return os.environ["HF_TOKEN"]
    env_path = Path("/Users/juanqui/Git/sotto/.env")
    if not env_path.exists():
        raise RuntimeError(f"no HF_TOKEN in env and {env_path} missing")
    for line in env_path.read_text().splitlines():
        line = line.strip()
        if line.startswith("HF_TOKEN="):
            token = line.split("=", 1)[1]
            os.environ["HF_TOKEN"] = token
            return token
    raise RuntimeError("no HF_TOKEN in .env")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--jsonl", type=Path, default=DEFAULT_JSONL)
    parser.add_argument("--commit-message", type=str, default=DEFAULT_MSG)
    parser.add_argument("--dataset", type=str, default=DATASET_ID)
    parser.add_argument("--dry-run", action="store_true",
                        help="Load + concat + validate but do NOT push")
    args = parser.parse_args()

    if not args.jsonl.exists():
        print(f"ERROR: jsonl not found: {args.jsonl}", file=sys.stderr)
        return 1

    token = load_hf_token()
    print(f"[upload] HF token loaded (len={len(token)})")

    from datasets import Dataset, DatasetDict, load_dataset, concatenate_datasets

    print(f"[upload] loading existing dataset: {args.dataset}")
    existing = load_dataset(args.dataset, token=token)
    print(f"[upload] existing splits: {list(existing.keys())}")
    for split_name, split in existing.items():
        print(f"  {split_name}: {len(split):,} rows, features={split.features}")

    # Load new samples from JSONL
    print(f"[upload] reading new samples from {args.jsonl}")
    new_rows: list[dict] = []
    with open(args.jsonl) as f:
        for line_no, line in enumerate(f, 1):
            line = line.strip()
            if not line:
                continue
            try:
                d = json.loads(line)
            except json.JSONDecodeError as e:
                print(f"[upload] bad json line {line_no}: {e}", file=sys.stderr)
                continue
            if "input" not in d or "output" not in d:
                print(f"[upload] missing field line {line_no}", file=sys.stderr)
                continue
            # Keep only the two expected columns
            new_rows.append({"input": d["input"], "output": d["output"]})
    print(f"[upload] loaded {len(new_rows):,} new rows")
    if not new_rows:
        print("ERROR: no valid new rows to upload", file=sys.stderr)
        return 1

    # Sanity: every new row must contain a paragraph break
    bad = [i for i, r in enumerate(new_rows) if "\n\n" not in r["output"]]
    if bad:
        print(f"ERROR: {len(bad)} rows missing \\n\\n in output (first 3: {bad[:3]})", file=sys.stderr)
        return 1
    print(f"[upload] all {len(new_rows)} rows contain paragraph breaks in output")

    # Build Dataset, concatenate into train only
    new_ds = Dataset.from_list(new_rows)
    # Ensure same features schema
    new_ds = new_ds.cast(existing["train"].features)
    print(f"[upload] new dataset features: {new_ds.features}")

    combined_train = concatenate_datasets([existing["train"], new_ds])
    print(f"[upload] combined train: {len(existing['train']):,} + {len(new_ds):,} = {len(combined_train):,}")

    # Rebuild DatasetDict preserving other splits
    out = DatasetDict()
    for split_name, split in existing.items():
        if split_name == "train":
            out["train"] = combined_train
        else:
            out[split_name] = split
    print(f"[upload] final DatasetDict:")
    for split_name, split in out.items():
        print(f"  {split_name}: {len(split):,} rows")

    if args.dry_run:
        print("[upload] --dry-run: stopping before push_to_hub")
        return 0

    print(f"[upload] pushing to hub: {args.dataset}")
    print(f"[upload] commit msg    : {args.commit_message}")
    commit = out.push_to_hub(
        args.dataset,
        token=token,
        commit_message=args.commit_message,
    )
    print(f"[upload] push complete")
    # push_to_hub may return a CommitInfo or None depending on version
    if commit is not None:
        print(f"[upload] commit info: {commit}")
    print(f"[upload] dataset URL: https://huggingface.co/datasets/{args.dataset}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
