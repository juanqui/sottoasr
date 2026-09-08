import { beforeEach, describe, expect, it, vi } from 'vitest';
vi.mock('../utils/tauri', () => ({ getSettings: vi.fn(), updateSettings: vi.fn() }));
import { getSettings, updateSettings } from '../utils/tauri';
import { SettingsStore, createDefaultSettings } from './settings.svelte';
import type { Settings, UpdateSettingsResult } from '../utils/tauri';
const deferred = <T>() => { let resolve!: (value: T) => void; let reject!: (error: Error) => void; const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; }); return { promise, resolve, reject }; };
let store: SettingsStore;
beforeEach(() => { vi.resetAllMocks(); store = new SettingsStore(); vi.mocked(getSettings).mockResolvedValue(createDefaultSettings()); });

describe('settings draft lifecycle', () => {
  it('migrates missing fields conservatively and preserves explicit choices', async () => {
    vi.mocked(getSettings).mockResolvedValueOnce({ show_overlay:false } as Settings);
    await store.load();
    expect(store.current.llm_cleanup_enabled).toBe(false);
    expect(store.current.dictionary).toEqual([]);
    expect(store.current.vocabulary).toEqual([]);
    expect(store.current.show_overlay).toBe(false);
    vi.mocked(getSettings).mockResolvedValueOnce({ ...createDefaultSettings(), llm_cleanup_enabled:true, vocabulary:['Qwen'] });
    await store.load();
    expect(store.current.llm_cleanup_enabled).toBe(true);
    expect(store.current.vocabulary).toEqual(['Qwen']);
  });
  it('does not make fallback defaults editable after a failed load', async () => {
    vi.mocked(getSettings).mockRejectedValueOnce(new Error('unreadable'));
    expect(await store.load()).toBe(false);
    expect(store.loaded).toBe(false);
    await expect(store.save()).rejects.toThrow('Load settings');
    expect(updateSettings).not.toHaveBeenCalled();
    await store.load();
    expect(store.loaded).toBe(true);
    expect(store.error).toBe('');
  });
  it('ignores old load completions and completions after disposal', async () => {
    const first = deferred<Settings>();
    vi.mocked(getSettings).mockReturnValueOnce(first.promise);
    const old = store.load();
    await store.load();
    first.resolve({ ...createDefaultSettings(), vocabulary:['Stale'] });
    await old;
    expect(store.current.vocabulary).toEqual([]);
    const last = deferred<Settings>();
    vi.mocked(getSettings).mockReturnValueOnce(last.promise);
    const loading = store.load(); store.invalidateLoad(); last.resolve({ ...createDefaultSettings(), vocabulary:['Late'] }); await loading;
    expect(store.current.vocabulary).toEqual([]);
  });
  it('acknowledges only the submitted snapshot and shares an in-flight save', async () => {
    await store.load();
    store.update('vocabulary', ['Qwen']);
    const write = deferred<UpdateSettingsResult>();
    vi.mocked(updateSettings).mockReturnValueOnce(write.promise);
    const saving = store.save();
    expect(store.save()).toBe(saving);
    store.update('vocabulary', ['Qwen', 'Juanqui']);
    write.resolve({ settings:{ ...createDefaultSettings(), vocabulary:['Qwen'] }, warnings:['Shortcut unavailable'] });
    await saving;
    expect(updateSettings).toHaveBeenCalledTimes(1);
    expect(vi.mocked(updateSettings).mock.calls[0][0].vocabulary).toEqual(['Qwen']);
    expect(store.current.vocabulary).toEqual(['Qwen', 'Juanqui']);
    expect(store.saved?.vocabulary).toEqual(['Qwen']);
    expect(store.dirty).toBe(true);
    expect(store.warnings).toEqual(['Shortcut unavailable']);
    store.discard();
    expect(store.current.vocabulary).toEqual(['Qwen']);
    expect(store.dirty).toBe(false);
  });
  it('retains the draft and saved preference on write failure', async () => {
    await store.load(); store.update('llm_cleanup_enabled', true);
    vi.mocked(updateSettings).mockRejectedValueOnce(new Error('Disk full'));
    await expect(store.save()).rejects.toThrow('Disk full');
    expect(store.current.llm_cleanup_enabled).toBe(true);
    expect(store.saved?.llm_cleanup_enabled).toBe(false);
    expect(store.saving).toBe(false);
    expect(store.error).toContain('Disk full');
  });
});
