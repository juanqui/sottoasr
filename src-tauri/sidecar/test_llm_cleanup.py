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


class FakeTokenizer:
    bos_token = '<s>'

    def __init__(self):
        self.calls = []

    def apply_chat_template(self, messages, **kwargs):
        self.calls.append((messages, kwargs))
        return '<s>rendered'

    def encode(self, text, **kwargs):
        self.calls.append((text, kwargs))
        return list(range(10 if text.startswith('<s>') else 4))


class CleanupProtocolTests(unittest.TestCase):
    def fake_runtime(self, stream, context=131072):
        mx = ModuleType('mlx.core')
        mx.random = SimpleNamespace(seed=MagicMock())
        mx.clear_cache = MagicMock()
        mlx = ModuleType('mlx')
        mlx.core = mx
        lm = ModuleType('mlx_lm')
        lm.stream_generate = stream
        tokenizer = FakeTokenizer()
        stack = __import__('contextlib').ExitStack()
        stack.enter_context(patch.dict(sys.modules, {'mlx': mlx, 'mlx.core': mx, 'mlx_lm': lm}))
        stack.enter_context(patch.object(cleanup, 'load_model'))
        stack.enter_context(patch.object(cleanup, '_model', object()))
        stack.enter_context(patch.object(cleanup, '_warmed', True))
        stack.enter_context(patch.object(cleanup, '_tokenizer', tokenizer))
        stack.enter_context(patch.object(cleanup, '_sampler', object()))
        stack.enter_context(patch.object(cleanup, '_context_limit', context))
        alarm = stack.enter_context(patch.object(cleanup.signal, 'setitimer'))
        stack.enter_context(patch.object(cleanup.signal, 'signal', return_value=None))
        return stack, mx, tokenizer, alarm

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

    def test_generation_requires_stop_and_keeps_complete_exact_text(self):
        for reason in ('length', None, 'unknown'):
            with self.subTest(reason=reason), self.assertRaises(cleanup.CleanupError):
                cleanup.parse_generation('A valid-looking prefix.', reason)
        self.assertEqual(cleanup.parse_generation('', 'stop'), '')
        self.assertEqual(cleanup.parse_generation('\nExact tail.\n', 'stop'), '\nExact tail.\n')

    def test_utf8_byte_limit_not_character_limit(self):
        for text in ('語' * 10666 + 'ab', '🧭' * 8000, '\x00' * 32000):
            self.assertEqual(cleanup.validate_text(text), 32000)
            with self.assertRaises(cleanup.CleanupError):
                cleanup.validate_text(text + 'x')
        for invalid in (None, 5, '\ud800'):
            with self.subTest(invalid=repr(invalid)), self.assertRaises(cleanup.CleanupError):
                cleanup.validate_text(invalid)

    def test_oversized_input_rejected_before_model_load(self):
        with patch.object(cleanup, 'load_model', side_effect=AssertionError('must not load')):
            response = cleanup.safe_response({'action': 'cleanup', 'text': '語' * 10667})
        self.assertEqual(response['error_code'], 'text_limit')
        self.assertNotIn('text', response)

    def test_complete_stream_includes_final_tail_and_explicit_token_budget(self):
        calls, closed = [], []
        def stream(*args, **kwargs):
            calls.append(kwargs)
            try:
                yield SimpleNamespace(text='Keep ', finish_reason=None)
                yield SimpleNamespace(text='the final words.', finish_reason='stop')
            finally:
                closed.append(True)
        stack, mx, tokenizer, alarm = self.fake_runtime(stream)
        with stack:
            response = cleanup.safe_response({'action': 'cleanup', 'text': 'Keep um the final words.'})
        self.assertTrue(response['ok'])
        self.assertEqual(response['text'], 'Keep the final words.')
        self.assertEqual(response['finish_reason'], 'stop')
        self.assertEqual(calls[0]['prompt'], list(range(10)))
        self.assertEqual(calls[0]['max_tokens'], 128)
        self.assertNotIn('prompt_cache', calls[0])
        self.assertIn(('<s>rendered', {'add_special_tokens': False}), tokenizer.calls)
        self.assertEqual(closed, [True])
        mx.random.seed.assert_called_once_with(42)
        mx.clear_cache.assert_called_once_with()
        self.assertEqual(alarm.call_args_list[-1].args[1], 0)

    def test_context_overflow_never_starts_generation(self):
        stream = MagicMock(side_effect=AssertionError('must not generate'))
        stack, _, _, _ = self.fake_runtime(stream, context=100)
        with stack:
            response = cleanup.safe_response({'action': 'cleanup', 'text': 'Keep the complete source.'})
        self.assertEqual(response['error_code'], 'context_limit')
        stream.assert_not_called()

    def test_load_warms_native_generation_once_and_reports_ready(self):
        generated, closed = [], []
        def stream(*_args, **kwargs):
            generated.append(kwargs)
            try:
                yield SimpleNamespace(text='Synthetic readiness complete.', finish_reason='stop')
            finally:
                closed.append(True)
        stack, mx, tokenizer, _ = self.fake_runtime(stream)
        with stack, patch.object(cleanup, '_warmed', False), \
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
        self.assertTrue(after['loaded'])
        self.assertTrue(after['warmed'])
        self.assertEqual(after['status'], 'ready')
        self.assertEqual(len(generated), 1)
        self.assertEqual(closed, [True])
        self.assertIn((cleanup.WARMUP_TEXT, {}), tokenizer.calls)
        mx.clear_cache.assert_called_once_with()

    def test_failed_warmup_stays_unready_and_next_load_can_retry(self):
        attempts = []
        def stream(*_args, **_kwargs):
            attempts.append(True)
            yield SimpleNamespace(text='prefix', finish_reason='length' if len(attempts) == 1 else 'stop')
        stack, _, _, _ = self.fake_runtime(stream)
        with stack, patch.object(cleanup, '_warmed', False), \
                patch.object(cleanup, 'cached_model_path', return_value=Path('/synthetic/snapshot')):
            failure = cleanup.safe_response({'action': 'load'})
            status = cleanup.handle_request({'action': 'status'})
            self.assertFalse(cleanup._warmed)
            recovered = cleanup.handle_request({'action': 'load'})
        self.assertEqual(failure['error_code'], 'incomplete_generation')
        self.assertFalse(status['loaded'])
        self.assertFalse(status['warmed'])
        self.assertTrue(recovered['warmed'])
        self.assertTrue(recovered['did_warm'])
        self.assertEqual(len(attempts), 2)

    def test_direct_cleanup_warms_with_synthetic_data_then_uses_user_input(self):
        generated = []
        def stream(*_args, **_kwargs):
            generated.append(True)
            yield SimpleNamespace(text='Synthetic output' if len(generated) == 1 else 'Actual output', finish_reason='stop')
        stack, mx, tokenizer, _ = self.fake_runtime(stream)
        with stack, patch.object(cleanup, '_warmed', False):
            response = cleanup.handle_request({'action': 'cleanup', 'text': 'Actual um input'})
        self.assertEqual(response['text'], 'Actual output')
        self.assertEqual(len(generated), 2)
        sources = [entry[0] for entry in tokenizer.calls if isinstance(entry[0], str) and entry[0] != '<s>rendered']
        self.assertEqual(sources, [cleanup.WARMUP_TEXT, 'Actual um input'])
        self.assertEqual(mx.clear_cache.call_count, 2)

    def test_truncation_exception_and_overflow_never_return_partial_text(self):
        for mode in ('length', 'exception', 'overflow'):
            closed = []
            def stream(*_args, **_kwargs):
                try:
                    yield SimpleNamespace(text='private prefix', finish_reason=None)
                    if mode == 'exception':
                        raise RuntimeError('secret dictation appeared in a library exception')
                    yield SimpleNamespace(text='x' * 32000 if mode == 'overflow' else '', finish_reason='length')
                finally:
                    closed.append(True)
            stack, mx, _, _ = self.fake_runtime(stream)
            with self.subTest(mode=mode), stack:
                response = cleanup.safe_response({'action': 'cleanup', 'text': 'private prefix and the required ending'})
            self.assertFalse(response['ok'])
            self.assertNotIn('text', response)
            self.assertNotIn('private', json.dumps(response))
            self.assertNotIn('secret', json.dumps(response))
            self.assertEqual(closed, [True])
            mx.clear_cache.assert_called_once_with()

    def test_deferred_deadline_does_not_accept_completed_result(self):
        def stream(*_args, **_kwargs):
            yield SimpleNamespace(text='Complete.', finish_reason='stop')
        stack, _, _, _ = self.fake_runtime(stream)
        with stack, patch.object(cleanup.time, 'perf_counter', side_effect=[0.0, 10.01]):
            response = cleanup.safe_response({'action': 'cleanup', 'text': 'Complete.'})
        self.assertEqual(response['error_code'], 'timeout')
        self.assertNotIn('text', response)

    def test_control_and_multibyte_json_roundtrip_within_wire_limit(self):
        for text in ('\x00' * 32000, '🧭' * 8000, '語' * 10666 + 'ab'):
            line = cleanup.encode_response({'ok': True, 'text': text, 'finish_reason': 'stop', 'elapsed_ms': 1})
            self.assertLessEqual(len(line), cleanup.MAX_LINE_BYTES)
            self.assertEqual(json.loads(line)['text'], text)
            self.assertTrue(line.endswith(b'\n'))
        oversized = cleanup.encode_response({'ok': True, 'text': 'x' * cleanup.MAX_LINE_BYTES})
        self.assertEqual(json.loads(oversized)['error_code'], 'response_limit')

    def test_bounded_input_stops_instead_of_reparsing_a_remainder(self):
        class Input(io.BytesIO):
            def readline(self, size=-1):
                self.last_size = size
                return super().readline(size)
        source = Input(b'x' * (cleanup.MAX_LINE_BYTES + 1) + b'\n{"action":"quit"}\n')
        output = io.BytesIO()
        with patch.object(cleanup, 'handle_request', side_effect=AssertionError('must not dispatch')):
            cleanup.serve(source, output)
        self.assertEqual(source.last_size, cleanup.MAX_LINE_BYTES + 1)
        self.assertEqual(len(output.getvalue().splitlines()), 1)
        self.assertEqual(json.loads(output.getvalue())['error_code'], 'request_limit')

    def test_invalid_requests_never_echo_private_data(self):
        source = io.BytesIO(b'{private malformed\n["private"]\n{"action":"private-action"}\n{"action":"quit"}\n')
        output = io.BytesIO()
        cleanup.serve(source, output)
        self.assertNotIn(b'private', output.getvalue())
        responses = [json.loads(line) for line in output.getvalue().splitlines()]
        self.assertEqual([row['ok'] for row in responses], [False, False, False, True])

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


if __name__ == '__main__':
    unittest.main()
