import { afterEach, expect, it, vi } from 'vitest';
import { flushSync, mount, unmount } from 'svelte';
import HistoryItem from './history-item.svelte';
import type { Transcription } from '../utils/tauri';

let component: ReturnType<typeof mount> | undefined;
let target: HTMLDivElement;

afterEach(async () => {
  if (component) await unmount(component);
  component = undefined;
  target?.remove();
});

it('exposes raw text and differences for dictionary edits with AI disabled', () => {
  const item: Transcription = {
    id: 'dictionary-test', text: 'Use Qwen next.', raw_text: 'Use Quen next.',
    duration_ms: 1000, created_at: '2026-09-07T12:00:00Z', word_count: 3,
    llm_applied: false, llm_cleanup_status: { kind: 'disabled' },
  };
  const oncopy = vi.fn();
  target = document.createElement('div');
  document.body.append(target);
  component = mount(HistoryItem, { target, props: { item, oncopy, ondelete: vi.fn() } });
  flushSync();
  const button = (label: string) => Array.from(target.querySelectorAll('button'))
    .find((element) => element.textContent?.trim() === label)!;

  expect(target.textContent).toContain('Dictionary applied');
  expect(target.textContent).not.toContain('AI Cleaned');
  button('Raw').click();
  flushSync();
  button('Copy transcript').click();
  expect(oncopy).toHaveBeenCalledWith('Use Qwen next.');

  button('Diff').click();
  flushSync();
  expect(target.querySelector('.diff-removed')?.textContent).toBe('Quen');
  expect(target.querySelector('.diff-added')?.textContent).toBe('Qwen');
});

it('shows interrupted capture distinctly while keeping its retained text copyable', () => {
  const item: Transcription = {
    id: 'interrupted', text: 'The words captured before unplugging.', capture_error: 'Microphone disconnected.',
    duration_ms: 1200, created_at: '2026-09-08T12:00:00Z', word_count: 5,
  };
  const oncopy = vi.fn();
  target = document.createElement('div'); document.body.append(target);
  component = mount(HistoryItem, { target, props: { item, oncopy, ondelete: vi.fn() } }); flushSync();
  expect(target.textContent).toContain('Interrupted');
  expect(target.textContent).toContain('Microphone disconnected.');
  expect(target.textContent).not.toContain('Cancelled');
  Array.from(target.querySelectorAll('button')).find((button) => button.textContent?.trim() === 'Copy')!.click();
  expect(oncopy).toHaveBeenCalledWith(item.text);
});

it('contains a harmful deletion in an explicit suggestion with separate provenance and copy feedback', async () => {
  const item: Transcription = {
    id: 'suggested', text: 'Qwen said the word um in the note.', raw_text: 'Quen said the word um in the note.',
    cleanup_suggestion: 'Qwen said the word in the note.', llm_applied: false,
    llm_cleanup_status: { kind: 'suggested', detail: { elapsed_ms: 20 } },
    duration_ms: 1000, created_at: '2026-09-08T12:00:00Z', word_count: 9,
  };
  let rejectCopy = true;
  const oncopy = vi.fn(async () => { if (rejectCopy) throw new Error('Clipboard unavailable'); });
  target = document.createElement('div'); document.body.append(target);
  component = mount(HistoryItem, { target, props: { item, oncopy, ondelete: vi.fn(), expanded: true, viewMode: 'diff' } }); flushSync();
  const button = (label: string) => Array.from(target.querySelectorAll('button')).find((node) => node.textContent?.trim() === label)!;

  expect(target.textContent).toContain('Suggestion to review');
  expect(target.textContent).not.toContain('AI Cleaned');
  expect(target.querySelector('.diff-removed')?.textContent).toBe('Quen');
  expect(target.querySelector('.suggestion-diff del')?.textContent).toContain('um');
  button('Copy suggestion').click(); await Promise.resolve(); await Promise.resolve(); flushSync();
  expect(oncopy).toHaveBeenLastCalledWith(item.cleanup_suggestion);
  expect(target.textContent).not.toContain('Suggestion copied');

  rejectCopy = false;
  button('Copy suggestion').click(); await Promise.resolve(); await Promise.resolve(); flushSync();
  expect(target.textContent).toContain('Suggestion copied');
  expect(button('Copy transcript')).toBeTruthy();
  button('Copy transcript').click(); await Promise.resolve(); await Promise.resolve(); flushSync();
  expect(oncopy).toHaveBeenLastCalledWith(item.text);
  expect(item.text).toBe('Qwen said the word um in the note.');
  expect(item.raw_text).toBe('Quen said the word um in the note.');
  expect(item.llm_applied).toBe(false);
});

it('renders a suggestion without claiming dictionary edits or interpreting transcript markup', () => {
  const item: Transcription = {
    id: 'plain-suggestion', text: 'Keep literal <b> um in this example.',
    cleanup_suggestion: 'Keep literal <b> in this example.',
    duration_ms: 1000, created_at: '2026-09-08T12:00:00Z', word_count: 7,
  };
  target = document.createElement('div'); document.body.append(target);
  component = mount(HistoryItem, { target, props: { item, oncopy: vi.fn(), ondelete: vi.fn() } }); flushSync();
  expect(target.textContent).toContain(item.text);
  expect(target.textContent).not.toContain('Dictionary applied');
  expect(target.querySelector('.suggestion')).toBeNull();
  (target.querySelector('.item-body') as HTMLButtonElement).click(); flushSync();
  expect(target.querySelector('.suggestion-diff')?.textContent).toContain('<b>');
  expect(target.querySelector('b')).toBeNull();
});

it.each([
  ['skipped_no_candidates', 'AI suggestions did not run: no edits qualified, or the transcript exceeded cleanup limits.'],
  ['no_changes', 'Cleanup made no changes.'],
] as const)('explains %s neutrally when the History row is expanded', (kind, explanation) => {
  const item: Transcription = {
    id: kind, text: 'Keep this complete transcript.', duration_ms: 1000,
    created_at: '2026-09-08T12:00:00Z', word_count: 4, llm_cleanup_status: { kind },
  };
  target = document.createElement('div'); document.body.append(target);
  component = mount(HistoryItem, { target, props: { item, oncopy: vi.fn(), ondelete: vi.fn() } }); flushSync();
  expect(target.textContent).not.toContain(explanation);
  (target.querySelector('.item-body') as HTMLButtonElement).click(); flushSync();
  expect(target.querySelector('.cleanup-explanation')?.textContent).toBe(explanation);
  expect(target.querySelector('.cleanup-fail-badge')).toBeNull();
});
