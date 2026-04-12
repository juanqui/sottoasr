import { describe, it, expect, vi, beforeEach } from 'vitest';

// Mock the tauri wrapper functions used by SettingsStore
vi.mock('../utils/tauri', () => ({
  getSettings: vi.fn(),
  updateSettings: vi.fn(),
}));

import { getSettings, updateSettings } from '../utils/tauri';
import { settingsStore } from './settings.svelte';

import type { Settings } from '../utils/tauri';

const mockGetSettings = vi.mocked(getSettings);
const mockUpdateSettings = vi.mocked(updateSettings);

const DEFAULT_SETTINGS: Settings = {
  push_to_talk_shortcut: 'CommandOrControl+Shift+Space',
  push_to_talk_shortcut_alt: null,
  toggle_shortcut: 'CommandOrControl+Shift+D',
  toggle_shortcut_alt: null,
  cancel_shortcut: 'Escape',
  cancel_shortcut_alt: null,
  show_overlay: true,
  auto_paste: true,
  restore_clipboard: true,
  restore_focus_before_paste: true,
  model_path: '',
  language: 'auto',
  max_history: 500,
  launch_at_login: false,
  llm_cleanup_enabled: false,
  auto_check_updates: true,
};

beforeEach(() => {
  // Reset mocks
  mockGetSettings.mockReset();
  mockUpdateSettings.mockReset();
  // Reset the store to default state
  settingsStore.current = { ...DEFAULT_SETTINGS };
  settingsStore.loaded = false;
  settingsStore.saving = false;
});

// ---------------------------------------------------------------------------
// load()
// ---------------------------------------------------------------------------
describe('load()', () => {
  it('loads settings from the backend and merges with defaults', async () => {
    const fetched: Settings = {
      ...DEFAULT_SETTINGS,
      show_overlay: false,
      max_history: 100,
    };
    mockGetSettings.mockResolvedValueOnce(fetched);

    await settingsStore.load();

    expect(mockGetSettings).toHaveBeenCalledOnce();
    expect(settingsStore.current.show_overlay).toBe(false);
    expect(settingsStore.current.max_history).toBe(100);
    expect(settingsStore.loaded).toBe(true);
  });

  it('falls back to defaults on error', async () => {
    mockGetSettings.mockRejectedValueOnce(new Error('backend unavailable'));

    await settingsStore.load();

    expect(settingsStore.current).toEqual(DEFAULT_SETTINGS);
    expect(settingsStore.loaded).toBe(true);
  });
});

// ---------------------------------------------------------------------------
// save()
// ---------------------------------------------------------------------------
describe('save()', () => {
  it('persists current settings to the backend', async () => {
    mockUpdateSettings.mockResolvedValueOnce(undefined);
    settingsStore.current = { ...DEFAULT_SETTINGS, language: 'en' };

    await settingsStore.save();

    expect(mockUpdateSettings).toHaveBeenCalledWith(
      expect.objectContaining({ language: 'en' }),
    );
    expect(settingsStore.saving).toBe(false);
  });

  it('sets saving flag during save and clears it on success', async () => {
    let savingDuringSave = false;
    mockUpdateSettings.mockImplementationOnce(async () => {
      savingDuringSave = settingsStore.saving;
    });

    await settingsStore.save();

    expect(savingDuringSave).toBe(true);
    expect(settingsStore.saving).toBe(false);
  });

  it('clears saving flag and rethrows on error', async () => {
    mockUpdateSettings.mockRejectedValueOnce(new Error('write failed'));

    await expect(settingsStore.save()).rejects.toThrow('write failed');
    expect(settingsStore.saving).toBe(false);
  });
});

// ---------------------------------------------------------------------------
// update()
// ---------------------------------------------------------------------------
describe('update()', () => {
  it('updates a single field immutably', () => {
    const before = settingsStore.current;
    settingsStore.update('language', 'es');

    expect(settingsStore.current.language).toBe('es');
    // Should be a new object (immutable update)
    expect(settingsStore.current).not.toBe(before);
  });

  it('preserves other fields when updating one', () => {
    settingsStore.update('max_history', 999);
    expect(settingsStore.current.max_history).toBe(999);
    expect(settingsStore.current.show_overlay).toBe(true);
    expect(settingsStore.current.push_to_talk_shortcut).toBe('CommandOrControl+Shift+Space');
  });
});
