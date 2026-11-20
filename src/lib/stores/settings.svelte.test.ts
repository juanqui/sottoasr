import { beforeEach, describe, expect, it, vi } from 'vitest';
vi.mock('../utils/tauri', () => ({ getSettings: vi.fn(), updateSettings: vi.fn(), getSettingsLoadError: vi.fn(), resetSettings: vi.fn() }));
import { getSettings, getSettingsLoadError, resetSettings, updateSettings } from '../utils/tauri';
import { SettingsStore, createDefaultSettings } from './settings.svelte';
import type { Settings, UpdateSettingsResult } from '../utils/tauri';
const deferred = <T>() => { let resolve!: (value: T) => void; let reject!: (error: Error) => void; const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; }); return { promise, resolve, reject }; };
/** The backend skips `None` options when it serializes settings. */
const asSerialized = (settings: Settings) => Object.fromEntries(Object.entries(settings).filter(([, value]) => value !== null)) as Settings;
let store: SettingsStore;
beforeEach(() => { vi.resetAllMocks(); store = new SettingsStore(); vi.mocked(getSettings).mockResolvedValue(createDefaultSettings()); vi.mocked(getSettingsLoadError).mockResolvedValue(null); });

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
  it('treats a no-op edit after Save as clean when the backend omits empty alternates', async () => {
    vi.mocked(getSettings).mockResolvedValueOnce(asSerialized(createDefaultSettings()));
    await store.load();
    expect(store.dirty).toBe(false);
    store.update('show_overlay', false);
    vi.mocked(updateSettings).mockImplementationOnce(async (settings) => ({ settings: asSerialized(settings), warnings: [] }));
    await store.save();
    expect(store.dirty).toBe(false);
    // Clearing an alternate that is already empty writes null.
    store.update('toggle_shortcut_alt', null);
    store.update('input_device', null);
    expect(store.dirty).toBe(false);
  });
});

describe('replacement rows', () => {
  it('blocks Save while a row has only one field filled in', async () => {
    await store.load();
    store.update('dictionary', [{ heard: 'Quen', replacement: '  ' }]);
    expect(store.validationError).toContain('incomplete replacement');
    await expect(store.save()).rejects.toThrow('incomplete replacement');
    expect(updateSettings).not.toHaveBeenCalled();
    expect(store.saving).toBe(false);
  });
  it('submits trimmed rows and drops rows with both fields empty', async () => {
    await store.load();
    store.update('dictionary', [{ heard: ' Quen ', replacement: 'Qwen ' }, { heard: ' ', replacement: '' }]);
    expect(store.validationError).toBe('');
    vi.mocked(updateSettings).mockImplementationOnce(async (settings) => ({ settings, warnings: [] }));
    await store.save();
    expect(vi.mocked(updateSettings).mock.calls[0][0].dictionary).toEqual([{ heard: 'Quen', replacement: 'Qwen' }]);
    expect(store.current.dictionary).toEqual([{ heard: 'Quen', replacement: 'Qwen' }]);
    expect(store.dirty).toBe(false);
  });
});

describe('speech engine', () => {
  const aws = { profile:'work', region:null, language:'en-US', vocabulary_name:null };
  it('defaults to the local engine, also for a file without the engine keys', async () => {
    expect(createDefaultSettings().speech_engine).toBe('local');
    expect(createDefaultSettings().aws_transcribe).toEqual({ profile:null, region:null, language:'en-US', vocabulary_name:null });
    const { speech_engine: _engine, aws_transcribe: _aws, ...older } = createDefaultSettings();
    vi.mocked(getSettings).mockResolvedValueOnce(older as Settings);
    await store.load();
    expect(store.current.speech_engine).toBe('local');
    expect(store.current.aws_transcribe).toEqual(createDefaultSettings().aws_transcribe);
    expect(store.dirty).toBe(false);
  });
  it('treats a no-op AWS edit as clean when the backend omits the empty AWS options', async () => {
    // The backend omits `None` values and may write the keys in another order.
    vi.mocked(getSettings).mockResolvedValueOnce({ ...createDefaultSettings(), speech_engine:'aws_transcribe', aws_transcribe:{ language:'en-US', profile:'work' } });
    await store.load();
    expect(JSON.stringify(store.current.aws_transcribe)).toBe(JSON.stringify(aws));
    store.update('aws_transcribe', { ...store.current.aws_transcribe, vocabulary_name:null });
    expect(store.dirty).toBe(false);
    store.update('aws_transcribe', { ...store.current.aws_transcribe, region:'us-west-2' });
    vi.mocked(updateSettings).mockImplementationOnce(async (settings) => ({ settings:{ ...settings, aws_transcribe:{ language:'en-US', region:'us-west-2', profile:'work' } }, warnings:[] }));
    await store.save();
    expect(store.dirty).toBe(false);
    store.update('aws_transcribe', { ...store.current.aws_transcribe, vocabulary_name:null });
    expect(store.dirty).toBe(false);
  });
  it('blocks Save while AWS Transcribe has no profile', async () => {
    await store.load();
    store.update('speech_engine', 'aws_transcribe');
    expect(store.validationError).toBe('Choose an AWS profile for AWS Transcribe.');
    await expect(store.save()).rejects.toThrow('Choose an AWS profile for AWS Transcribe.');
    expect(updateSettings).not.toHaveBeenCalled();
    store.update('aws_transcribe', { ...store.current.aws_transcribe, profile:'  ' });
    expect(store.validationError).toBe('Choose an AWS profile for AWS Transcribe.');
    store.update('aws_transcribe', aws);
    expect(store.validationError).toBe('');
    // The local engine needs no profile.
    store.update('aws_transcribe', createDefaultSettings().aws_transcribe);
    store.update('speech_engine', 'local');
    expect(store.validationError).toBe('');
  });
  it('sends trimmed AWS options, with an empty value as null', async () => {
    await store.load();
    store.update('speech_engine', 'aws_transcribe');
    store.update('aws_transcribe', { profile:' work ', region:'', language:'en-US', vocabulary_name:'  ' });
    vi.mocked(updateSettings).mockImplementationOnce(async (settings) => ({ settings, warnings:[] }));
    await store.save();
    expect(vi.mocked(updateSettings).mock.calls[0][0].aws_transcribe).toEqual(aws);
    expect(store.current.aws_transcribe).toEqual(aws);
    expect(store.dirty).toBe(false);
  });
});

describe('unreadable settings file', () => {
  it('offers a reset only when the backend reports why loading failed', async () => {
    vi.mocked(getSettings).mockRejectedValueOnce('Settings unavailable');
    vi.mocked(getSettingsLoadError).mockResolvedValueOnce('expected value at line 1 column 1');
    expect(await store.load()).toBe(false);
    expect(store.loadError).toBe('expected value at line 1 column 1');
    vi.mocked(resetSettings).mockResolvedValueOnce(asSerialized(createDefaultSettings()));
    expect(await store.reset()).toBe(true);
    expect(store.loaded).toBe(true);
    expect(store.loadError).toBeNull();
    expect(store.dirty).toBe(false);
  });
  it('keeps the reset available when the backend refuses it', async () => {
    vi.mocked(getSettings).mockRejectedValue('Settings unavailable');
    vi.mocked(getSettingsLoadError).mockResolvedValue('trailing characters');
    await store.load();
    vi.mocked(resetSettings).mockRejectedValueOnce('Finish dictation before resetting settings.');
    expect(await store.reset()).toBe(false);
    expect(store.loaded).toBe(false);
    expect(store.loadError).toBe('trailing characters');
    expect(store.error).toBe('Could not reset settings: Finish dictation before resetting settings.');
    expect(store.resetting).toBe(false);
  });
  it('shows the written defaults when the reset fails after writing them', async () => {
    vi.mocked(getSettings).mockRejectedValueOnce('Settings unavailable');
    vi.mocked(getSettingsLoadError).mockResolvedValueOnce('trailing characters');
    await store.load();
    // The backend wrote the defaults and cleared the load error, then could not register a shortcut.
    const written = asSerialized(createDefaultSettings());
    vi.mocked(resetSettings).mockRejectedValueOnce('Settings were reset, but keyboard shortcuts could not be turned on. Choose other shortcuts and save.');
    vi.mocked(getSettings).mockResolvedValueOnce(written);
    expect(await store.reset()).toBe(false);
    expect(store.loaded).toBe(true);
    expect(store.loadError).toBeNull();
    expect(store.dirty).toBe(false);
    expect(store.error).toBe('Settings were reset, but keyboard shortcuts could not be turned on. Choose other shortcuts and save.');
    expect(store.resetting).toBe(false);
  });
});
