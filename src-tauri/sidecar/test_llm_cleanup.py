"""Sidecar protocol and preparation tests; no model, network, or MLX dependency."""

import hashlib
import io
import json
import os
from pathlib import Path
import sys
import tempfile
from types import ModuleType, SimpleNamespace
import unittest
from unittest.mock import MagicMock, patch

import llm_cleanup as cleanup


EOS_IDS = [1, 130073]
HEAD_CACHE = object()  # identity sentinel for the shared immutable prefix cache


class FakeDetokenizer:
    """BPEStreamingDetokenizer stand-in: last_segment/finalize/reset semantics."""

    def __init__(self, pieces=None, final_append='', on_finalize=None):
        self.pieces = pieces if pieces is not None else {}
        self.final_append = final_append
        self.on_finalize = on_finalize  # raise-on-finalize (alarm-race tests)
        self.text = ''
        self.last_segment = ''
        self.added = []

    def reset(self):
        self.text = ''
        self.last_segment = ''
        self.added = []

    def add_token(self, token_id):
        piece = self.pieces.get(token_id, f'<{token_id}>')
        self.added.append(token_id)
        self.last_segment = piece
        self.text += piece

    def finalize(self):
        if callable(self.on_finalize):
            self.on_finalize(self)  # may raise: models an alarm mid-finalize
        elif self.on_finalize is not None:
            raise self.on_finalize
        if self.final_append:
            self.text += self.final_append


class FakeTokenizer:
    bos_token = '<s>'
    eos_token_ids = EOS_IDS

    def __init__(self, pieces=None, final_append='', on_finalize=None):
        self.calls = []
        self.pieces = pieces or {}
        self.final_append = final_append
        self.on_finalize = on_finalize

    def apply_chat_template(self, messages, **kwargs):
        self.calls.append((messages, kwargs))
        return '<s>rendered'

    def encode(self, text, **kwargs):
        self.calls.append((text, kwargs))
        # Prompt encodings are a fixed 10-token id prefix; KEY encodings are
        # one token per character so budget-formula boundaries are observable.
        return list(range(10 if text.startswith('<s>') else len(text)))

    @property
    def detokenizer(self):
        return FakeDetokenizer(self.pieces, self.final_append, self.on_finalize)


def R(uid, token, reason):
    return SimpleNamespace(uid=uid, token=token, finish_reason=reason)


class FakeBatchGenerator:
    """Scripted mlx_lm BatchGenerator; records ctor/insert/remove/close.

    Each scripted batch entry is either a result list or a callable invoked
    with the generator (returning the list) — used to move the fake clock.
    """

    def __init__(self, recorder, batches):
        self.recorder = recorder
        self.batches = batches
        self.next_calls = 0

    def insert(self, prompts, max_tokens=None, caches=None):
        self.recorder['insert'] = {'prompts': prompts, 'max_tokens': list(max_tokens),
                                   'caches': list(caches)}
        return list(range(len(prompts)))

    def next_generated(self):
        index = self.next_calls
        self.next_calls += 1
        if index < len(self.batches):
            entry = self.batches[index]
            return entry(self) if callable(entry) else entry
        return []

    def remove(self, uids):
        self.recorder.setdefault('removed', []).extend(uids)

    def close(self):
        self.recorder['closed'] = True


class Clock:
    def __init__(self):
        self.t = 0.0

    def perf_counter(self):
        return self.t


class CleanupProtocolTests(unittest.TestCase):
    def fake_runtime(self, batches, pieces=None, final_append='', head_ids=None,
                     context=131072, warmed=True, on_finalize=None):
        mx = ModuleType('mlx.core')
        mx.random = SimpleNamespace(seed=MagicMock())
        mx.clear_cache = MagicMock()
        mx.array = MagicMock(side_effect=lambda value: value)
        mx.eval = MagicMock()
        mx.synchronize = MagicMock()
        mlx = ModuleType('mlx')
        mlx.core = mx
        lm = ModuleType('mlx_lm')
        generate = ModuleType('mlx_lm.generate')
        recorder = {'ctor': [], 'make_cache': []}
        self.generator_batches = batches

        def factory(model, **kwargs):
            recorder['ctor'].append(kwargs)
            return FakeBatchGenerator(recorder, self.generator_batches)

        generate.BatchGenerator = factory
        generate.generate_step = MagicMock(return_value=iter(()))
        lm.generate = generate
        models = ModuleType('mlx_lm.models')
        cache_module = ModuleType('mlx_lm.models.cache')

        def make_prompt_cache(_model):
            marker = object()
            recorder['make_cache'].append(marker)
            return marker

        cache_module.make_prompt_cache = make_prompt_cache
        models.cache = cache_module
        lm.models = models
        tokenizer = FakeTokenizer(pieces, final_append, on_finalize)
        recorder['tokenizer'] = tokenizer
        recorder['clock'] = Clock()
        stack = __import__('contextlib').ExitStack()
        stack.enter_context(patch.dict(sys.modules, {
            'mlx': mlx, 'mlx.core': mx, 'mlx_lm': lm, 'mlx_lm.generate': generate,
            'mlx_lm.models': models, 'mlx_lm.models.cache': cache_module}))
        stack.enter_context(patch.object(cleanup, 'load_model'))
        stack.enter_context(patch.object(cleanup, 'build_head'))
        stack.enter_context(patch.object(cleanup, '_model', object()))
        stack.enter_context(patch.object(cleanup, '_warmed', warmed))
        stack.enter_context(patch.object(cleanup, '_tokenizer', tokenizer))
        stack.enter_context(patch.object(cleanup, '_sampler', object()))
        stack.enter_context(patch.object(cleanup, '_context_limit', context))
        stack.enter_context(patch.object(cleanup, '_head_ids', [] if head_ids is None else head_ids))
        stack.enter_context(patch.object(cleanup, '_prefix_cache', HEAD_CACHE))
        stack.enter_context(patch.object(cleanup, '_detok_template', tokenizer.detokenizer))
        stack.enter_context(patch.object(cleanup, 'time', SimpleNamespace(
            perf_counter=recorder['clock'].perf_counter)))
        alarm = stack.enter_context(patch.object(cleanup.signal, 'setitimer'))
        stack.enter_context(patch.object(cleanup.signal, 'signal', return_value=None))
        return stack, mx, tokenizer, alarm, recorder

    def test_d7_prompt_matches_frozen_config_and_native_layout(self):
        frozen = Path(__file__).resolve().parents[2] / 'benchmarks/llm/model-study-2026-09-08/direct-cleanup-diagnostic/development/d5-restart/prompt-d7.json'
        self.assertEqual(hashlib.sha256(frozen.read_bytes()).hexdigest(), cleanup.PROMPT_SHA256)
        self.assertEqual(json.loads(frozen.read_text()), cleanup.PROMPT)
        tokenizer = FakeTokenizer()
        with patch.object(cleanup, '_tokenizer', tokenizer):
            self.assertEqual(cleanup.build_prompt('Keep this ending.'), '<s>rendered')
        messages, options = tokenizer.calls[0]
        self.assertEqual([message['role'] for message in messages], ['system', 'user'])
        self.assertEqual(messages[1]['content'], '<transcript>\nKeep this ending.\n</transcript>')
        self.assertEqual(messages[0]['content'].count('<example>'), 5)
        self.assertIn('\n\n<examples>\n<example>\nInput:\n<transcript>\n', messages[0]['content'])
        self.assertIn('Please pack those blue spacers for tomorrow.', messages[0]['content'])
        self.assertIn('Set the field named um to zero.', messages[0]['content'])
        self.assertEqual(options, {'add_generation_prompt': True, 'tokenize': False, 'enable_thinking': False})

    def test_utf8_byte_limit_not_character_limit(self):
        # The serial `cleanup` action is gone; the byte-cap discipline now
        # lives in the per-item batch validator (MAX_INPUT_BYTES) and the
        # output cap (MAX_TEXT_BYTES). Char counts far below the byte cap
        # must still trip it: multi-byte content is measured in BYTES.
        cap = cleanup.MAX_INPUT_BYTES
        for text in ('語' * (cap // 3 - 1) + 'xxxx', '🧭' * (cap // 4), 'x' * cap):
            self.assertEqual(len(text.encode('utf-8')), cap)  # exactly at the cap
            self.assertEqual(cleanup.validate_batch_text(text), cap)
            with self.assertRaises(cleanup.CleanupError):
                cleanup.validate_batch_text(text + 'x')
        # Oversized single code points and non-text stay typed failures.
        for invalid in (None, 5, '\ud800'):
            with self.subTest(invalid=repr(invalid)), self.assertRaises(cleanup.CleanupError):
                cleanup.validate_batch_text(invalid)

    def test_zero_text_batch_is_a_noop_before_any_model_work(self):
        guard = AssertionError('must not touch the model or generator')
        with patch.object(cleanup, 'load_model', side_effect=guard), \
             patch.object(cleanup, 'warm_model', side_effect=guard), \
             patch.object(cleanup, 'build_head', side_effect=guard), \
             patch.object(cleanup, 'run_batch_generation', side_effect=guard):
            response = cleanup.safe_response({'action': 'cleanup_batch', 'texts': []})
        self.assertEqual(response, {'ok': True, 'results': []})

    def test_batch_contract_violations_are_typed_invalid_request(self):
        guard = AssertionError('must not reach the model')
        cases = {
            'over_16': ['x'] * 17,
            'over_cap': ['x' * (cleanup.MAX_INPUT_BYTES + 1)],
            'mixed_over_cap': ['keep um this', 'x' * (cleanup.MAX_INPUT_BYTES + 1)],
            'non_string': [5],
            'none_entry': [None],
        }
        for name, texts in cases.items():
            with self.subTest(case=name), \
                 patch.object(cleanup, 'load_model', side_effect=guard), \
                 patch.object(cleanup, 'warm_model', side_effect=guard), \
                 patch.object(cleanup, 'run_batch_generation', side_effect=guard):
                response = cleanup.safe_response({'action': 'cleanup_batch', 'texts': texts})
            self.assertFalse(response['ok'])
            self.assertEqual(response['error_code'], 'invalid_request')
            self.assertNotIn('results', response)
        for missing in ({'action': 'cleanup_batch'}, {'action': 'cleanup_batch', 'texts': 'x'}):
            with self.subTest(request=repr(missing)), \
                 patch.object(cleanup, 'run_batch_generation', side_effect=guard):
                response = cleanup.safe_response(dict(missing))
            self.assertEqual(response['error_code'], 'invalid_request')

    def test_batch_uses_head_cache_shared_greedy_shape_and_frozen_budget(self):
        # KEY-text budget boundaries: 40→128 floor, 48→128, 49→130, 4096→8192 cap.
        keys = ['k' * 40, 'k' * 48, 'k' * 49, 'k' * 4096]
        batches = [
            [R(i, 100 + i, None) for i in range(4)],
            [R(i, None, 'stop') for i in range(4)],
        ]
        stack, mx, tokenizer, alarm, recorder = self.fake_runtime(batches)
        with stack:
            response = cleanup.safe_response({'action': 'cleanup_batch', 'texts': keys})
        self.assertTrue(response['ok'])
        self.assertEqual([row['index'] for row in response['results']], [0, 1, 2, 3])
        self.assertTrue(all(row['status'] == 'ok' for row in response['results']))
        ctor = recorder['ctor'][0]
        self.assertEqual(ctor['stop_tokens'], [[eos] for eos in EOS_IDS])
        self.assertNotIn('max_tokens', ctor)
        self.assertEqual(ctor['completion_batch_size'], 4)
        self.assertEqual(ctor['prefill_batch_size'], 4)
        self.assertEqual(ctor['prefill_step_size'], 64)
        insert = recorder['insert']
        self.assertEqual(insert['max_tokens'], [128, 128, 130, 8192])
        # Head-matched rows dispatch through the shared immutable prefix cache.
        self.assertEqual(insert['prompts'], [list(range(10)) for _ in range(4)])
        self.assertTrue(all(entry is HEAD_CACHE for entry in insert['caches']))
        self.assertIn((keys[0], {}), tokenizer.calls)
        mx.random.seed.assert_called_once_with(42)
        mx.clear_cache.assert_called_once_with()
        self.assertEqual(alarm.call_args_list[-1].args[1], 0)

    def test_prefix_token_miss_dispatches_full_prompt_with_fresh_cache(self):
        # Prompt ids are [0..9]; an item whose ids don't start with the head
        # keeps the model judgment (full prompt) but loses the shared prefix.
        head_ids = [1, 2, 3]
        batches = [[R(0, 999, None)], [R(0, None, 'stop')]]
        stack, _, _, _, recorder = self.fake_runtime(batches, head_ids=head_ids,
                                                     pieces={999: 'Judged anyway.'})
        with stack:
            response = cleanup.safe_response({'action': 'cleanup_batch', 'texts': ['keep um this']})
        self.assertEqual(response['results'],
                         [{'index': 0, 'status': 'ok', 'text': 'Judged anyway.', 'elapsed_ms': 0}])
        insert = recorder['insert']
        self.assertEqual(insert['prompts'], [list(range(10))])  # FULL prompt, no tail cut
        self.assertEqual(len(recorder['make_cache']), 1)
        self.assertEqual(insert['caches'], [recorder['make_cache'][0]])

    def test_batch_results_are_a_permutation_with_out_of_order_completion(self):
        batches = [
            [R(2, 70, None)],
            [R(2, None, 'stop'), R(0, None, 'stop')],
            [R(1, None, 'stop')],
        ]
        stack, _, _, _, _ = self.fake_runtime(batches)
        with stack:
            response = cleanup.safe_response({'action': 'cleanup_batch',
                                              'texts': ['one um', 'two um', 'three um']})
        self.assertEqual(sorted(row['index'] for row in response['results']), [0, 1, 2])
        self.assertEqual(len(response['results']), 3)
        by_index = {row['index']: row for row in response['results']}
        self.assertTrue(all(row['status'] == 'ok' for row in by_index.values()))
        self.assertTrue(all('elapsed_ms' in row for row in by_index.values()))

    def test_stop_with_zero_output_tokens_is_a_valid_empty_proposal(self):
        batches = [[R(0, None, 'stop')]]
        stack, _, _, _, _ = self.fake_runtime(batches)
        with stack:
            response = cleanup.safe_response({'action': 'cleanup_batch', 'texts': ['um']})
        self.assertEqual(response['results'], [{'index': 0, 'status': 'ok', 'text': '', 'elapsed_ms': 0}])

    def test_partial_truncation_and_missing_items_are_never_proposed(self):
        for mode in ('length', 'abort', 'missing'):
            # A stop response carries the stop token itself, never content:
            # real content must arrive as its own unflagged segment first.
            kept = [R(1, 999, None), R(1, None, 'stop')]
            if mode == 'length':
                batches = [[R(0, 7, None)] + kept + [R(0, 66, 'length')]]
            elif mode == 'abort':
                batches = [[R(0, None, 'abort')] + kept]
            else:
                batches = [kept]  # uid 0 never finishes
            stack, mx, _, _, _ = self.fake_runtime(
                batches, pieces={7: 'private prefix', 66: 'tail', 999: 'kept words'})
            with self.subTest(mode=mode), stack:
                response = cleanup.safe_response({'action': 'cleanup_batch',
                                                  'texts': ['private prefix words', 'other um words']})
            self.assertTrue(response['ok'])
            rows = {row['index']: row for row in response['results']}
            self.assertEqual(sorted(rows), [0, 1])
            self.assertEqual(rows[0], {'index': 0, 'status': 'failed',
                                       'error_code': 'incomplete_generation'})
            self.assertNotIn('text', rows[0])
            self.assertEqual(rows[1], {'index': 1, 'status': 'ok', 'text': 'kept words', 'elapsed_ms': 0})
            self.assertNotIn('private', json.dumps(response))
            mx.clear_cache.assert_called_once_with()

    def test_runtime_exception_never_leaks_private_values(self):
        def boom(_gen):
            raise RuntimeError('SECRET transcript value')
        stack, _, _, _, recorder = self.fake_runtime([boom])
        stderr = io.StringIO()
        with stack, patch('sys.stderr', stderr):
            response = cleanup.safe_response({'action': 'cleanup_batch', 'texts': ['PRIVATE narration']})
        self.assertFalse(response['ok'])
        self.assertEqual(response['error_code'], 'operation_failed')
        self.assertNotIn('results', response)
        self.assertIn('RuntimeError', stderr.getvalue())
        self.assertIn('llm_cleanup.py:', stderr.getvalue())
        self.assertNotIn('SECRET', stderr.getvalue())
        self.assertNotIn('PRIVATE', stderr.getvalue())
        self.assertTrue(recorder.get('closed'))

    def test_eos_at_budget_normalizes_to_stop_and_non_eos_stays_rejected(self):
        pieces = {70: 'Cleaned.', 71: 'Trunc'}
        batches = [
            [R(0, 70, None), R(1, 71, None)],
            [R(0, 130073, 'length'), R(1, 66, 'length')],  # EOS-at-budget vs true truncation
        ]
        stack, _, _, _, _ = self.fake_runtime(batches, pieces=pieces)
        with stack:
            response = cleanup.safe_response({'action': 'cleanup_batch',
                                              'texts': ['clean um me', 'trunc um ate']})
        rows = {row['index']: row for row in response['results']}
        self.assertEqual(rows[0], {'index': 0, 'status': 'ok', 'text': 'Cleaned.', 'elapsed_ms': 0})
        self.assertEqual(rows[1], {'index': 1, 'status': 'failed',
                                   'error_code': 'incomplete_generation'})
        # The EOS id itself must never reach the detokenizer (no synthetic piece).
        self.assertNotIn('<130073>', rows[0]['text'])

    def test_context_overflow_fails_items_without_touching_the_generator(self):
        # prompt(10) + budget(≥128) > 100 ⇒ every item rejected pre-dispatch.
        stack, _, _, _, recorder = self.fake_runtime([[R(0, 999, 'stop')]], context=100)
        with stack:
            response = cleanup.safe_response({'action': 'cleanup_batch',
                                              'texts': ['x' * 48, 'small um one']})
        rows = {row['index']: row for row in response['results']}
        self.assertEqual(sorted(rows), [0, 1])
        for row in rows.values():
            self.assertEqual(row['status'], 'failed')
            self.assertEqual(row['error_code'], 'context_limit')
        self.assertEqual(recorder['ctor'], [])  # generator never constructed

    def test_deadline_seals_late_and_pending_items_as_timeout(self):
        pieces = {70: 'Early complete.', 71: 'Late', 72: 'Stuck'}
        def advance_then_finish(gen):
            gen.recorder['clock'].t = 10.01  # the deferred strike lands here
            return [R(2, None, 'stop')]      # completes AFTER the deadline ⇒ timeout
        batches = [
            [R(0, 70, None), R(1, 71, None), R(2, 72, None)],
            [R(0, None, 'stop')],   # item 0 seals ok before the deadline
            [R(1, 71, None)],       # keeps the drain alive for item 1
            advance_then_finish,
        ]
        stack, mx, _, alarm, recorder = self.fake_runtime(batches, pieces=pieces)
        with stack:
            response = cleanup.safe_response({'action': 'cleanup_batch',
                                              'texts': ['early um', 'late um', 'stuck um']})
        rows = {row['index']: row for row in response['results']}
        self.assertEqual(sorted(rows), [0, 1, 2])
        self.assertEqual(rows[0], {'index': 0, 'status': 'ok', 'text': 'Early complete.', 'elapsed_ms': 0})
        self.assertEqual(rows[1], {'index': 1, 'status': 'timeout'})  # pending at the seal
        self.assertEqual(rows[2], {'index': 2, 'status': 'timeout'})  # stop past the deadline
        self.assertTrue(recorder['closed'])
        mx.clear_cache.assert_called_once_with()
        self.assertEqual(alarm.call_args_list[-1].args[1], 0)

    def test_completed_before_deadline_survives_a_later_alarm_strike(self):
        def strike(_gen):
            raise TimeoutError()  # the SIGALRM handler interrupting the drain
        batches = [
            [R(0, 70, None), R(1, 71, None)],
            [R(0, None, 'stop')],  # seals ok before the strike
            strike,
        ]
        stack, _, _, _, recorder = self.fake_runtime(batches, pieces={70: 'Done before strike.'})
        with stack:
            response = cleanup.safe_response({'action': 'cleanup_batch',
                                              'texts': ['done um before strike', 'never um ends']})
        rows = {row['index']: row for row in response['results']}
        self.assertEqual(rows[0], {'index': 0, 'status': 'ok', 'text': 'Done before strike.', 'elapsed_ms': 0})
        self.assertEqual(rows[1], {'index': 1, 'status': 'timeout'})
        self.assertTrue(recorder['closed'])

    def test_alarm_during_preprocessing_still_seals_every_index(self):
        # The result vector is preallocated before any alarm-able work: a
        # strike in the encode/prompt loop must yield a COMPLETE response
        # (all timeouts), never a short one that the client retires on.
        stack, _, _, _, recorder = self.fake_runtime([[]])
        real_prompt_ids = cleanup.prompt_ids_for
        calls = []

        def strike_on_second(text):
            calls.append(text)
            if len(calls) == 2:
                raise TimeoutError()
            return real_prompt_ids(text)

        with stack, patch.object(cleanup, 'prompt_ids_for', side_effect=strike_on_second):
            response = cleanup.safe_response({'action': 'cleanup_batch',
                                              'texts': ['first um text', 'second um text']})
        self.assertEqual(response['ok'], True)
        self.assertEqual([row['index'] for row in response['results']], [0, 1])
        self.assertEqual([row['status'] for row in response['results']],
                         ['timeout', 'timeout'])
        self.assertEqual(recorder['ctor'], [])  # generation never started

    def test_alarm_during_eos_finalize_keeps_sibling_seals(self):
        # Item 1 seals ok first; item 0's finalize is interrupted MID-finalize
        # (after the slot was slated for sealing but before the seal stored).
        # Index 0 must still appear — as timeout — and index 1 keeps its ok.
        pieces = {70: 'Interrupted', 71: 'Sibling complete.'}
        batches = [
            [R(0, 70, None), R(1, 71, None)],
            [R(1, None, 'stop')],   # sibling seals ok BEFORE the strike
            [R(0, None, 'stop')],   # item 0's finalize raises underneath
        ]
        def interrupt(detok):
            if detok.text.startswith('Interrupted'):
                raise TimeoutError()
        stack, _, _, _, recorder = self.fake_runtime(
            batches, pieces=pieces, on_finalize=interrupt)
        with stack:
            response = cleanup.safe_response({'action': 'cleanup_batch',
                                              'texts': ['strike me um', 'finish me um']})
        rows = {row['index']: row for row in response['results']}
        self.assertEqual(sorted(rows), [0, 1])
        self.assertEqual(rows[0], {'index': 0, 'status': 'timeout'})
        self.assertEqual(rows[1], {'index': 1, 'status': 'ok',
                                   'text': 'Sibling complete.', 'elapsed_ms': 0})
        self.assertTrue(recorder['closed'])

    def test_deferred_clock_without_python_alarm_seals_stop_as_timeout(self):
        # Native work can carry the batch past the deadline while the SIGALRM
        # never lands (deferred signal). The stop branch's own clock recheck
        # — not the alarm — must demote the over-deadline completion.
        def advance(_gen):
            _gen.recorder['clock'].t = 10.01  # past REQUEST_TIMEOUT_SECONDS
            return [R(0, None, 'stop')]       # a legitimate stop, now late

        batches = [[R(0, 70, None)], advance]
        stack, _, _, alarm, _ = self.fake_runtime(batches, pieces={70: 'Late complete.'})
        with stack:
            response = cleanup.safe_response({'action': 'cleanup_batch',
                                              'texts': ['late under native work um']})
        self.assertEqual(response['results'], [{'index': 0, 'status': 'timeout'}])
        # Detection came from the stop branch's clock recheck, not the alarm:
        # the instrumented setitimer was only ever armed/disarmed.
        self.assertTrue(all(call.args[0] in (1, 0) for call in alarm.call_args_list))

    def test_output_byte_cap_removes_the_offending_uid_and_siblings_complete(self):
        # Cap is bytes, not characters: 'aé' is 3 B, '🧭' is 4 B (2 chars, 7 B total).
        pieces = {70: 'aé', 71: '🧭', 80: 'go', 81: '!'}
        batches = [
            [R(0, 70, None), R(1, 80, None)],
            [R(0, 71, None), R(1, 81, None)],  # item 0 now 7 B > 5 B cap; sibling is 3 B
            [R(1, None, 'stop')],
        ]
        stack, _, _, _, recorder = self.fake_runtime(batches, pieces=pieces)
        with stack, patch.object(cleanup, 'MAX_TEXT_BYTES', 5):
            response = cleanup.safe_response({'action': 'cleanup_batch',
                                              'texts': ['multibyte um overrun', 'sibling um text']})
        rows = {row['index']: row for row in response['results']}
        self.assertEqual(rows[0], {'index': 0, 'status': 'failed', 'error_code': 'text_limit'})
        self.assertNotIn('text', rows[0])
        self.assertEqual(rows[1], {'index': 1, 'status': 'ok', 'text': 'go!', 'elapsed_ms': 0})
        self.assertEqual(recorder.get('removed'), [0])

    def test_finalize_appended_bytes_are_rechecked_and_in_cap_text_ships(self):
        pieces = {70: 'aaa'}
        batches = [[R(0, 70, None)], [R(0, None, 'stop')]]
        # finalize() appends 4 more bytes ⇒ 7 B total over a 6 B cap ⇒ text_limit.
        stack, _, _, _, _ = self.fake_runtime(batches, pieces=pieces, final_append='dddd')
        with stack, patch.object(cleanup, 'MAX_TEXT_BYTES', 6):
            response = cleanup.safe_response({'action': 'cleanup_batch', 'texts': ['um overrun']})
        self.assertEqual(response['results'],
                         [{'index': 0, 'status': 'failed', 'error_code': 'text_limit'}])
        # Same path under the default cap: the finalized text ships.
        stack, _, _, _, _ = self.fake_runtime(batches, pieces=pieces, final_append='d')
        with stack:
            response = cleanup.safe_response({'action': 'cleanup_batch', 'texts': ['um overrun']})
        self.assertEqual(response['results'],
                         [{'index': 0, 'status': 'ok', 'text': 'aaad', 'elapsed_ms': 0}])

    def test_load_builds_head_then_warms_through_the_batch_path_once(self):
        batches = [
            [R(0, 70, None), R(1, 71, None)],
            [R(0, None, 'stop'), R(1, None, 'stop')],
        ]
        pieces = {70: 'Synthetic readiness complete.', 71: 'Second synthetic pass complete.'}
        stack, mx, tokenizer, _, recorder = self.fake_runtime(batches, pieces=pieces, warmed=False)
        build_order = []
        def fake_build_head():
            build_order.append('head')
        with stack, patch.object(cleanup, 'build_head', side_effect=fake_build_head), \
                patch.object(cleanup, 'cached_model_path', return_value=Path('/synthetic/snapshot')):
            before = cleanup.handle_request({'action': 'status'})
            self.assertFalse(before['loaded'])
            self.assertFalse(before['warmed'])
            first = cleanup.handle_request({'action': 'load'})
            second = cleanup.handle_request({'action': 'load'})
            after = cleanup.handle_request({'action': 'status'})
        self.assertTrue(first['warmed'])
        self.assertTrue(first['did_warm'])
        self.assertTrue(second['warmed'])
        self.assertFalse(second['did_warm'])
        self.assertEqual(after['status'], 'ready')
        self.assertEqual(len(recorder['ctor']), 1)  # ONE batch warmup, two prompts
        self.assertEqual(recorder['insert']['max_tokens'], [128, 136])  # 2*len(key)+32, floored
        sources = [entry[0] for entry in tokenizer.calls
                   if isinstance(entry[0], str) and not entry[0].startswith('<s>')]
        self.assertEqual(sources, [cleanup.WARMUP_TEXT, cleanup.WARMUP_TEXT_SECOND])
        mx.clear_cache.assert_called_once_with()

    def test_failed_warmup_stays_unready_and_next_load_can_retry(self):
        tries = []
        def first_pass(_gen):
            return [R(0, 70, None), R(1, 71, None)]
        def verdict(_gen):
            tries.append(True)
            if len(tries) == 1:
                return [R(0, None, 'stop'), R(1, 66, 'length')]  # item 1 truncates
            return [R(0, None, 'stop'), R(1, None, 'stop')]
        batches = [first_pass, verdict]
        stack, _, _, _, recorder = self.fake_runtime(batches, pieces={70: 'ok', 66: 'partial'},
                                                     warmed=False)
        with stack, patch.object(cleanup, 'cached_model_path', return_value=Path('/synthetic/snapshot')):
            failure = cleanup.safe_response({'action': 'load'})
            status = cleanup.handle_request({'action': 'status'})
            self.assertFalse(cleanup._warmed)
            recovered = cleanup.handle_request({'action': 'load'})
        self.assertEqual(failure['error_code'], 'incomplete_generation')
        self.assertFalse(status['loaded'])
        self.assertFalse(status['warmed'])
        self.assertTrue(recovered['warmed'])
        self.assertTrue(recovered['did_warm'])
        self.assertEqual(len(recorder['ctor']), 2)

    def test_cleanup_batch_ensures_warm_before_arming_the_alarm(self):
        order = []
        stack, _, _, alarm, _ = self.fake_runtime([[R(0, None, 'stop')]])
        def warm_then_record():
            order.append(('warm', alarm.call_count))
            return True
        with stack, patch.object(cleanup, '_warmed', False), \
                patch.object(cleanup, 'warm_model', side_effect=warm_then_record):
            response = cleanup.safe_response({'action': 'cleanup_batch', 'texts': ['keep um this']})
        self.assertTrue(response['ok'])
        self.assertEqual(order, [('warm', 0)])  # the alarm was still unarmed at ensure-warm
        self.assertGreaterEqual(alarm.call_count, 2)  # arm + disarm bracket the generation

    def test_control_and_multibyte_worst_case_fits_the_batch_response_cap(self):
        # JSON escaping is ≤ 6× raw bytes per text: 16 × 32 000 B of NUL must fit.
        nul = '\x00' * 32000
        response = {'ok': True, 'results': [
            {'index': i, 'status': 'ok', 'text': nul, 'elapsed_ms': 1} for i in range(16)]}
        line = cleanup.encode_response(response)
        self.assertLessEqual(len(line), cleanup.MAX_RESPONSE_LINE_BYTES)
        decoded = json.loads(line)
        self.assertEqual(decoded['results'][15]['text'], nul)
        self.assertTrue(line.endswith(b'\n'))
        for text in ('🧭' * 8000, '語' * 10666 + 'ab'):
            line = cleanup.encode_response({'ok': True, 'text': text, 'elapsed_ms': 1})
            self.assertLessEqual(len(line), cleanup.MAX_RESPONSE_LINE_BYTES)
            self.assertEqual(json.loads(line)['text'], text)
        oversized = cleanup.encode_response({'ok': True, 'text': 'x' * cleanup.MAX_RESPONSE_LINE_BYTES})
        self.assertEqual(json.loads(oversized)['error_code'], 'response_limit')

    def test_bounded_input_stops_instead_of_reparsing_a_remainder(self):
        class Input(io.BytesIO):
            def readline(self, size=-1):
                self.last_size = size
                return super().readline(size)
        source = Input(b'x' * (cleanup.MAX_REQUEST_LINE_BYTES + 1) + b'\n{"action":"quit"}\n')
        output = io.BytesIO()
        with patch.object(cleanup, 'handle_request', side_effect=AssertionError('must not dispatch')):
            cleanup.serve(source, output)
        self.assertEqual(source.last_size, cleanup.MAX_REQUEST_LINE_BYTES + 1)
        self.assertEqual(len(output.getvalue().splitlines()), 1)
        self.assertEqual(json.loads(output.getvalue())['error_code'], 'request_limit')

    def test_invalid_requests_never_echo_private_data(self):
        source = io.BytesIO(b'{private malformed\n["private"]\n{"action":"private-action"}\n{"action":"quit"}\n')
        output = io.BytesIO()
        cleanup.serve(source, output)
        self.assertNotIn(b'private', output.getvalue())
        responses = [json.loads(line) for line in output.getvalue().splitlines()]
        self.assertEqual([row['ok'] for row in responses], [False, False, False, True])

    def test_serial_cleanup_action_and_legacy_constants_are_gone(self):
        response = cleanup.safe_response({'action': 'cleanup', 'text': 'keep um this'})
        self.assertEqual(response['error_code'], 'invalid_action')
        self.assertFalse(hasattr(cleanup, 'cleanup_text'))
        self.assertFalse(hasattr(cleanup, 'parse_generation'))
        self.assertFalse(hasattr(cleanup, 'MAX_LINE_BYTES'))

    def test_fixed_revision_lookup_and_update_are_offline(self):
        calls = []
        def unavailable(*args, **kwargs):
            calls.append((args, kwargs))
            raise FileNotFoundError()
        hub = SimpleNamespace(snapshot_download=unavailable)
        with patch.dict(sys.modules, {'huggingface_hub': hub}):
            self.assertIsNone(cleanup.cached_model_path())
            response = cleanup.handle_request({'action': 'check_update'})
        self.assertFalse(response['update_available'])
        self.assertTrue(all(kwargs == {'revision': cleanup.MODEL_REVISION, 'local_files_only': True, 'token': False} for _, kwargs in calls))
        self.assertTrue(all(args == (cleanup.MODEL_ID,) for args, _ in calls))

    def model_fixture(self, root):
        data = {'model.safetensors': b'weights', 'chat_template.jinja': b'template', 'config.json': b'{}'}
        for name, contents in data.items():
            (root / name).write_bytes(contents)
        return {name: (len(contents), hashlib.sha256(contents).hexdigest()) for name, contents in data.items()}

    def test_preparation_hashes_once_and_cheap_readiness_detects_mtime_change(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            with patch.object(cleanup, 'MODEL_FILES', self.model_fixture(root)):
                marker = cleanup.verify_snapshot(root)
                self.assertEqual(marker['revision'], cleanup.MODEL_REVISION)
                self.assertTrue(cleanup.snapshot_is_verified(root))
                with patch.object(cleanup.hashlib, 'sha256', side_effect=AssertionError('must not hash')):
                    self.assertTrue(cleanup.snapshot_is_verified(root))
                weight = root / 'model.safetensors'
                stat = weight.stat()
                os.utime(weight, ns=(stat.st_atime_ns, stat.st_mtime_ns + 1_000_000))
                self.assertFalse(cleanup.snapshot_is_verified(root))

    def test_bad_hash_preserves_cached_files_and_previous_marker(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            with patch.object(cleanup, 'MODEL_FILES', self.model_fixture(root)):
                cleanup.verify_snapshot(root)
                previous = (root / cleanup.VERIFICATION_FILE).read_bytes()
                weight = root / 'model.safetensors'
                weight.write_bytes(b'changed')
                with self.assertRaises(cleanup.CleanupError):
                    cleanup.verify_snapshot(root)
                self.assertEqual(weight.read_bytes(), b'changed')
                self.assertEqual((root / cleanup.VERIFICATION_FILE).read_bytes(), previous)
                self.assertTrue((root / 'chat_template.jinja').is_file())

    def test_missing_template_or_malformed_marker_is_not_ready(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            with patch.object(cleanup, 'MODEL_FILES', self.model_fixture(root)):
                cleanup.verify_snapshot(root)
                (root / 'chat_template.jinja').rename(root / 'preserved-template')
                self.assertFalse(cleanup.snapshot_is_verified(root))
                (root / 'preserved-template').rename(root / 'chat_template.jinja')
                for contents in ('[]', '{', '{"schema_version":1}', 'x' * 16385):
                    (root / cleanup.VERIFICATION_FILE).write_text(contents)
                    self.assertFalse(cleanup.snapshot_is_verified(root))

    def test_explicit_preparation_never_follows_remote_main(self):
        calls = []
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            def download(*args, **kwargs):
                calls.append((args, kwargs))
                return str(root)
            with patch.object(cleanup, 'local_snapshot', return_value=None), \
                 patch.object(cleanup, 'verify_snapshot') as verify, \
                 patch.dict(sys.modules, {'huggingface_hub': SimpleNamespace(snapshot_download=download)}):
                cleanup.prepare_model()
            self.assertEqual(calls[0][1]['revision'], cleanup.MODEL_REVISION)
            self.assertFalse(calls[0][1]['token'])
            self.assertEqual(set(calls[0][1]['allow_patterns']), set(cleanup.MODEL_FILES))
            verify.assert_called_once_with(root)


def _qualified_model_cached():
    # The probe needs huggingface_hub; environments without it simply lack
    # the qualified cache, so the gate must skip, not error at collection.
    try:
        return cleanup.cached_model_path() is not None
    except ImportError:
        return False


@unittest.skipUnless(_qualified_model_cached(),
                     'qualified local cleanup model is not cached')
class RealBatchModelTests(unittest.TestCase):
    """End-to-end native proof on the pinned artifact (warm path, seconds)."""

    @classmethod
    def setUpClass(cls):
        response = cleanup.handle_request({'action': 'load'})
        assert response.get('ok') and response.get('warmed'), response

    def test_load_builds_the_computed_batch_head(self):
        # 365 is a MEASUREMENT pinned test-side (drift detection); production
        # build_head computes the common prefix without a service guard.
        self.assertEqual(len(cleanup._head_ids), 365)
        self.assertIsInstance(cleanup._prefix_cache, list)
        self.assertEqual(len(cleanup._prefix_cache), 42)  # per-layer KV caches
        second = cleanup.handle_request({'action': 'load'})
        self.assertFalse(second['did_warm'])

    def test_real_multi_text_batch_returns_one_ok_result_per_index(self):
        texts = [
            'I um need uh the the green notebook tomorrow.',
            'Please pack the uh um those blue spacers for tomorrow.',
            'Um, set the field named um to zero.',
            'Café 🧭 um报价 the the value is 1,024.',  # E8: non-ASCII is opaque
        ]
        response = cleanup.handle_request({'action': 'cleanup_batch', 'texts': texts})
        self.assertTrue(response['ok'], response)
        rows = {row['index']: row for row in response['results']}
        self.assertEqual(sorted(rows), list(range(len(texts))))
        for index, row in rows.items():
            self.assertEqual(row['status'], 'ok', row)
            self.assertIsInstance(row['text'], str)
            self.assertGreaterEqual(row['elapsed_ms'], 0)

    def test_real_single_text_batch_is_the_same_wire_shape(self):
        response = cleanup.handle_request({'action': 'cleanup_batch',
                                           'texts': ['the the um quick brown fox.']})
        rows = response['results']
        self.assertEqual(len(rows), 1)
        self.assertEqual(rows[0]['index'], 0)
        self.assertEqual(rows[0]['status'], 'ok')
        self.assertIsInstance(rows[0]['text'], str)


if __name__ == '__main__':
    unittest.main()
