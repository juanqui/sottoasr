# Automatic-cleanup qualification

- **Version:** 1.0
- **Date:** 2026-09-08
- **Status:** In Review

This is the separate D3 MiniCPM5-2B qualification driver. The original direct-model comparison remains immutable. The first manifest (`freeze.json`) records validator v4 and is **paused before any release data was opened or inference ran**: an independent code-delimiter probe found an existing protection-contract gap. The superseding [final v5 manifest](freeze-v5-final.json) pins the repaired parser and exact runtime before the root-authorized release evaluation. Earlier manifests remain archived as paused pre-run snapshots.

The driver matches native MLX-LM tokenization, checks full formatted prompt plus reserved generation against the loaded model's 131,072-token context limit before inference, and admits only complete outputs within the 10-second request deadline. Invalid, unfinished, or rejected proposals fall back to the complete source. Timing excludes loading and serialization; validation is timed separately. No production model or installed settings are changed.

The canonical span scorer and its 13-test evidence are archived here. Gold-equivalent duplicate occurrences are aligned explicitly; partial removals do not count as complete deletion runs or complete cleanup. Unscored ambiguity cannot be omitted to claim a passing release. Exact protected payload checks and independent semantic review supplement lexical metrics. The known reported sentence's canonical gold differs from its accepted natural alternative, so that warmup is assessed separately and excluded from release scores.

The v5 quote/code protection tests also pass under the actual qualification Python 3.14.5 / Unicode 16.0.0 runtime: [24 test groups](v5-python314-tests.txt). The embedded release protected-span annotations are source-only metadata used for independent scoring, never model inputs or validator hints.

Reproduce only with the pinned local model and isolated dependency versions:

```bash
HF_HUB_OFFLINE=1 HF_HUB_DISABLE_IMPLICIT_TOKEN=1 PYTHONDONTWRITEBYTECODE=1 python run_qualification.py \
  --model /absolute/path/to/pinned/MiniCPM5-2B-MLX --label minicpm2000-D3-v5-release250 \
  --prompt prompt-inline.json --cases release250.json --warmup-cases reported-only.json \
  --modes few_shot --output /tmp/qualification-release250.json
PYTHONDONTWRITEBYTECODE=1 python evaluate_qualification.py \
  --gold release250.json --results /tmp/qualification-release250.json \
  --validator ../validation-prototype-v5/source_validation.py --embedded-protected \
  --out /tmp/qualification-release250-evaluated.json
```

Run the separate `release-restarts12.json` with a distinct output path. The archived experiment ran each dataset once after its freeze; future reproduction is an already-seen regression measurement, not a new blind qualification.
