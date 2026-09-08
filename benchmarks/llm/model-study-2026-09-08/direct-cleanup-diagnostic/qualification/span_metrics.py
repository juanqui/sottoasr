"""Offline source-word deletion metrics; no model/runtime dependencies.

Usage:
  python span_metrics.py --gold semantic60.json --outputs outputs.json \
      --output-field output --out scored-spans.json

Gold must be deletion-only after NFC, casefold and word tokenization. Exact
punctuation/identifier fidelity belongs to the independent protected-string
scorer. Occurrence alignment is optimistic among equally valid source matches;
it measures lexical editing, not semantic certainty. Ambiguity is bounded and
reported as unscored rather than resolved with an arbitrary greedy match.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import unicodedata
from pathlib import Path
from typing import Sequence

MAX_WORDS = 1024
MAX_ALIGNMENTS = 4096
MAX_VISITED_STATES = 250_000
MAX_ALIGNMENT_PAIRS = 250_000
WORD_PATTERN = re.compile(r"\w+(?:['’]\w+)*", re.UNICODE)


class AlignmentLimitError(ValueError):
    pass


def words(text: str) -> tuple[str, ...]:
    normalized = unicodedata.normalize('NFC', text).casefold()
    return tuple(token.replace('’', "'") for token in WORD_PATTERN.findall(normalized))


def lcs_table(source: Sequence[str], target: Sequence[str]) -> list[list[int]]:
    if len(source) > MAX_WORDS or len(target) > MAX_WORDS:
        raise AlignmentLimitError(f'Word limit exceeded ({MAX_WORDS})')
    table = [[0] * (len(target) + 1) for _ in range(len(source) + 1)]
    for i in range(len(source) - 1, -1, -1):
        for j in range(len(target) - 1, -1, -1):
            match = 1 + table[i + 1][j + 1] if source[i] == target[j] else 0
            table[i][j] = max(match, table[i + 1][j], table[i][j + 1])
    return table


def source_alignment_masks(
    source: Sequence[str], target: Sequence[str], table: list[list[int]],
) -> set[int]:
    """All distinct optimal LCS source-occurrence masks, within explicit bounds.

    Output additions are skipped by the LCS and counted separately. Distinct
    output alignments retaining the same source occurrences share one mask.
    """
    pending = [(0, 0, 0)]
    visited = set()
    masks = set()
    while pending:
        i, j, mask = pending.pop()
        state = (i, j, mask)
        if state in visited:
            continue
        visited.add(state)
        if len(visited) > MAX_VISITED_STATES:
            raise AlignmentLimitError('Alignment state limit exceeded')
        if table[i][j] == 0:
            masks.add(mask)
            if len(masks) > MAX_ALIGNMENTS:
                raise AlignmentLimitError('Equivalent alignment limit exceeded')
            continue
        best = table[i][j]
        if i < len(source) and table[i + 1][j] == best:
            pending.append((i + 1, j, mask))
        if j < len(target) and table[i][j + 1] == best:
            pending.append((i, j + 1, mask))
        if (i < len(source) and j < len(target) and source[i] == target[j]
                and 1 + table[i + 1][j + 1] == best):
            pending.append((i + 1, j + 1, mask | (1 << i)))
    return masks


def deletion_runs(length: int, retained_mask: int) -> list[tuple[int, int, int]]:
    """Half-open word ranges and masks for contiguous deleted source runs."""
    result = []
    start = None
    for i in range(length + 1):
        deleted = i < length and not retained_mask & (1 << i)
        if deleted and start is None:
            start = i
        elif not deleted and start is not None:
            mask = ((1 << (i - start)) - 1) << start
            result.append((start, i, mask))
            start = None
    return result


def score_case(raw: str, expected: str, output: str) -> dict:
    source, gold, result = words(raw), words(expected), words(output)
    gold_lcs = lcs_table(source, gold)
    if gold_lcs[0][0] != len(gold):
        raise ValueError('Gold is not a deletion-only source-word subsequence')
    output_lcs = lcs_table(source, result)
    required_lcs = lcs_table(gold, result)
    additions = len(result) - output_lcs[0][0]
    required_losses = len(gold) - required_lcs[0][0]
    valid_preservation = additions == 0 and required_losses == 0
    wanted = len(source) - len(gold)
    base = {
        'source_words': len(source), 'gold_words': len(gold), 'output_words': len(result),
        'gold_delete_tokens': wanted,
        'source_word_additions': additions,
        'required_word_losses': required_losses,
        'preservation_valid': valid_preservation,
        'lexically_complete': result == gold,
        'complete_cleanup': wanted > 0 and result == gold,
        'source_words_deleted': len(source) - output_lcs[0][0],
    }
    try:
        gold_masks = source_alignment_masks(source, gold, gold_lcs)
        output_masks = source_alignment_masks(source, result, output_lcs)
        if len(gold_masks) * len(output_masks) > MAX_ALIGNMENT_PAIRS:
            raise AlignmentLimitError('Alignment pair limit exceeded')
        all_words = (1 << len(source)) - 1
        run_options = {mask: deletion_runs(len(source), mask) for mask in gold_masks}
        minimum_gold_runs = min(len(runs) for runs in run_options.values())
        best_tokens = None
        best_runs = None
        for gold_mask in gold_masks:
            wanted_mask = all_words ^ gold_mask
            runs = run_options[gold_mask]
            for output_mask in output_masks:
                removed_mask = all_words ^ output_mask
                correct = (wanted_mask & removed_mask).bit_count()
                completed = sum(mask & output_mask == 0 for _, _, mask in runs)
                # Token credit is invariant to equivalent repeated copies.
                rank = (correct, -len(runs), completed, -gold_mask, -output_mask)
                if best_tokens is None or rank > best_tokens[0]:
                    best_tokens = (rank, gold_mask, output_mask, runs)
                # The run denominator depends ONLY on source and gold. Among
                # minimum-run gold alignments, maximize fully removed runs.
                # Token and run alignments may differ, so expose both counts.
                if len(runs) == minimum_gold_runs:
                    run_rank = (completed, correct, -gold_mask, -output_mask)
                    if best_runs is None or run_rank > best_runs[0]:
                        best_runs = (run_rank, gold_mask, output_mask, runs)
        assert best_tokens is not None and best_runs is not None
        token_rank, token_gold_mask, token_output_mask, token_runs = best_tokens
        run_rank, _, output_mask, runs = best_runs
        correct = token_rank[0]
        completed = run_rank[0]
        return {
            **base, 'score_status': 'scored',
            'correctly_deleted_tokens': correct,
            'safe_correctly_deleted_tokens': correct if valid_preservation else 0,
            'deletion_token_recall': correct / wanted if wanted else None,
            'safe_deletion_token_recall': correct / wanted if wanted and valid_preservation else (0.0 if wanted else None),
            'gold_delete_runs': len(runs), 'completed_delete_runs': completed,
            'safe_completed_delete_runs': completed if valid_preservation else 0,
            'deletion_run_recall': completed / len(runs) if runs else None,
            'alignment_required_words_removed': (token_gold_mask & ~token_output_mask).bit_count(),
            'token_alignment_gold_run_count': len(token_runs),
            'run_alignment_correctly_deleted_tokens': run_rank[1],
            'gold_alignment_count': len(gold_masks), 'output_alignment_count': len(output_masks),
            'gold_runs': [
                {'start_word': start, 'end_word': end, 'words': list(source[start:end]),
                 'completed': mask & output_mask == 0}
                for start, end, mask in runs
            ],
        }
    except AlignmentLimitError as error:
        return {**base, 'score_status': 'unscored_alignment_limit', 'reason': str(error)}


def summarize(rows: list[dict]) -> dict:
    scored = [row for row in rows if row['score_status'] == 'scored']
    needed = [row for row in rows if row['gold_delete_tokens'] > 0]
    unchanged_gold = [row for row in rows if row['gold_delete_tokens'] == 0]
    token_denominator = sum(row['gold_delete_tokens'] for row in rows)
    correct = sum(row['correctly_deleted_tokens'] for row in scored)
    safe_correct = sum(row['safe_correctly_deleted_tokens'] for row in scored)
    run_denominator = sum(row['gold_delete_runs'] for row in scored)
    return {
        'cases': len(rows), 'scored_cases': len(scored),
        'unscored_cases': len(rows) - len(scored),
        'edit_required_cases': len(needed),
        'complete_cleanup_cases': sum(row['complete_cleanup'] for row in needed),
        'complete_cleanup_rate': sum(row['complete_cleanup'] for row in needed) / len(needed) if needed else None,
        'preserve_cases': len(unchanged_gold),
        'preserve_lexically_unchanged': sum(row['lexically_complete'] for row in unchanged_gold),
        'invalid_preservation_cases': sum(not row['preservation_valid'] for row in rows),
        'source_word_additions': sum(row['source_word_additions'] for row in rows),
        'required_word_losses': sum(row['required_word_losses'] for row in rows),
        'gold_delete_tokens_all_cases': token_denominator,
        'correctly_deleted_tokens_scored_cases': correct,
        'safe_correctly_deleted_tokens_scored_cases': safe_correct,
        'deletion_token_recall_lower_bound': correct / token_denominator if token_denominator else None,
        'safe_deletion_token_recall_lower_bound': safe_correct / token_denominator if token_denominator else None,
        'gold_delete_runs_scored_cases': run_denominator,
        'completed_delete_runs_scored_cases': sum(row['completed_delete_runs'] for row in scored),
        'deletion_run_recall_scored_cases': sum(row['completed_delete_runs'] for row in scored) / run_denominator if run_denominator else None,
        'safe_deletion_run_recall_scored_cases': sum(row['safe_completed_delete_runs'] for row in scored) / run_denominator if run_denominator else None,
        'run_metrics_cover_all_cases': len(scored) == len(rows),
    }


def load_rows(path: Path, key: str | None) -> list[dict]:
    payload = json.loads(path.read_text())
    if key is not None:
        payload = payload[key]
    elif isinstance(payload, dict):
        candidates = [payload[name] for name in ['results', 'rows', 'cases'] if isinstance(payload.get(name), list)]
        if len(candidates) != 1:
            raise ValueError('Specify --rows-key for this output envelope')
        payload = candidates[0]
    if not isinstance(payload, list) or not all(isinstance(row, dict) for row in payload):
        raise ValueError('Expected a list of case objects')
    return payload


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--gold', type=Path, required=True)
    parser.add_argument('--outputs', type=Path, required=True)
    parser.add_argument('--output-field', default='output')
    parser.add_argument('--rows-key')
    parser.add_argument('--out', type=Path, required=True)
    args = parser.parse_args()
    if args.out.resolve() in {args.gold.resolve(), args.outputs.resolve()}:
        raise ValueError('Scoring output must not overwrite an input artifact')
    gold_rows = load_rows(args.gold, None)
    output_rows = load_rows(args.outputs, args.rows_key)
    output_by_id = {}
    for row in output_rows:
        if row['id'] in output_by_id:
            raise ValueError(f'Duplicate output ID: {row["id"]}')
        output_by_id[row['id']] = row
    seen = set()
    scores = []
    for case in gold_rows:
        case_id = case['id']
        if case_id in seen:
            raise ValueError(f'Duplicate gold ID: {case_id}')
        seen.add(case_id)
        model_row = output_by_id[case_id]
        output = model_row[args.output_field]
        if not isinstance(output, str):
            raise ValueError(f'Output is not text: {case_id}')
        metrics = score_case(case['raw'], case['expected'], output)
        scores.append({'id': case_id, 'group': case.get('group', case.get('category', 'unspecified')), **metrics})
    groups = sorted({row['group'] for row in scores})
    report = {
        'metric_version': 1,
        'gold_sha256': hashlib.sha256(args.gold.read_bytes()).hexdigest(),
        'outputs_sha256': hashlib.sha256(args.outputs.read_bytes()).hexdigest(),
        'scorer_sha256': hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
        'output_field': args.output_field,
        'limits': {'max_words': MAX_WORDS, 'max_alignments': MAX_ALIGNMENTS,
                   'max_visited_states': MAX_VISITED_STATES, 'max_alignment_pairs': MAX_ALIGNMENT_PAIRS},
        'notes': [
            'Word-level lexical scores do not replace protected-string fidelity or semantic review.',
            'Token recall maximizes correct deletions over equivalent optimal source alignments.',
            'Run recall uses the source/gold minimum-run denominator, fixed independently of model output, and maximizes completed runs among those alignments.',
            'Token and run occurrence alignments may differ; their supporting counts are reported separately.',
            'Contiguous runs use word positions; punctuation and whitespace alone do not split a run.',
            'Additions/required-word losses are independent preservation failures; safe recall credits neither case.',
            'A partial run earns token credit, never complete-run or complete-cleanup credit.',
            'Unscored ambiguity contributes zero token credit; run totals explicitly cover only scored cases.',
        ],
        'summary': summarize(scores),
        'groups': {group: summarize([row for row in scores if row['group'] == group]) for group in groups},
        'ignored_extra_output_ids': sorted(set(output_by_id) - seen),
        'results': scores,
    }
    args.out.write_text(json.dumps(report, ensure_ascii=False, indent=2) + '\n')
    print(json.dumps(report['summary'], indent=2))


if __name__ == '__main__':
    main()
