import { beforeEach, expect, it, vi } from 'vitest';
vi.mock('../utils/tauri', () => ({ getTranscriptions:vi.fn(), deleteTranscription:vi.fn(), clearTranscriptions:vi.fn() }));
import { getTranscriptions, deleteTranscription, clearTranscriptions } from '../utils/tauri';
import { TranscriptionStore } from './transcriptions.svelte';
const item = (id:string) => ({id,text:id,created_at:'2026-09-08T00:00:00Z',duration_ms:1000,word_count:1});
beforeEach(() => vi.resetAllMocks());
it('keeps existing history on load failure and exposes the error', async () => {
  const store = new TranscriptionStore(); store.items = [item('saved')]; vi.mocked(getTranscriptions).mockRejectedValue(new Error('Unreadable'));
  await store.load(); expect(store.items).toHaveLength(1); expect(store.error).toContain('Unreadable'); expect(store.loaded).toBe(false);
});
it('does not remove entries after failed persistence', async () => {
  const store = new TranscriptionStore(); store.items = [item('saved')]; vi.mocked(deleteTranscription).mockRejectedValue(new Error('Disk full')); vi.mocked(clearTranscriptions).mockRejectedValue(new Error('Disk full'));
  await expect(store.delete('saved')).rejects.toThrow('Disk full'); await expect(store.clear()).rejects.toThrow('Disk full'); expect(store.items).toHaveLength(1);
});
it('uses backend-confirmed clear IDs and preserves concurrent new entries', async () => {
  const store = new TranscriptionStore(); store.items = [item('old')];
  let resolve!: (ids:string[])=>void; vi.mocked(clearTranscriptions).mockReturnValue(new Promise((yes) => {resolve=yes;}));
  const clear = store.clear(); store.add(item('new')); resolve(['old']); await clear;
  expect(store.items.map((value) => value.id)).toEqual(['new']); store.add(item('old')); expect(store.items).toHaveLength(1);
});
it('deduplicates events that arrive during the first load', async () => {
  const store = new TranscriptionStore(); let resolve!: (items:ReturnType<typeof item>[])=>void;
  vi.mocked(getTranscriptions).mockReturnValue(new Promise((yes) => {resolve=yes;}));
  const load = store.load(); store.add(item('new')); resolve([item('new'),item('old')]); await load;
  expect(store.items).toHaveLength(2);
});
it('accepts authoritative rollover and preserves only new arrivals during reload', async () => {
  const store = new TranscriptionStore(); store.items = [item('evicted'), item('durable')];
  let resolve!: (items:ReturnType<typeof item>[])=>void;
  vi.mocked(getTranscriptions).mockReturnValue(new Promise((yes) => {resolve=yes;}));
  const load = store.load(); store.add(item('new')); resolve([item('durable')]); await load;
  expect(store.items.map((value) => value.id).sort()).toEqual(['durable','new']);
});
it('removes only acknowledged durable evictions without globally pruning legacy or unsaved rows', async () => {
  const store = new TranscriptionStore(); store.items = Array.from({length:5001},(_,i)=>item(`legacy-${i}`));
  store.add(item('unsaved')); expect(store.items).toHaveLength(5002);
  let resolve!: (items:ReturnType<typeof item>[])=>void;
  vi.mocked(getTranscriptions).mockReturnValue(new Promise((yes) => {resolve=yes;}));
  const load = store.load(); store.add({...item('durable-new'),removed_ids:['legacy-0']});
  resolve([item('legacy-0'),item('legacy-1'),item('unsaved')]); await load;
  expect(store.items.map((value)=>value.id).sort()).toEqual(['durable-new','legacy-1','unsaved']);
  expect(store.items[0]).not.toHaveProperty('removed_ids');
});
it('applies durable eviction acknowledgements even when their new record was already deleted', async () => {
  const store = new TranscriptionStore(); store.items = [item('newest'), item('oldest'), item('other')];
  vi.mocked(deleteTranscription).mockResolvedValue(undefined);
  await store.delete('newest');
  store.add({...item('newest'),removed_ids:['oldest']});
  expect(store.items.map((value)=>value.id)).toEqual(['other']);
});
