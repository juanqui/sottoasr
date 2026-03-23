import {
  getSettings as fetchSettings,
  updateSettings as saveSettings,
} from '../utils/tauri';

import type { Settings } from '../utils/tauri';

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
  llm_markdown_mode: false,
  llm_model_size: '2b',
};

class SettingsStore {
  /** Current settings */
  current: Settings = $state({ ...DEFAULT_SETTINGS });

  /** Whether the store has been loaded from the backend */
  loaded: boolean = $state(false);

  /** Whether settings are currently being saved */
  saving: boolean = $state(false);

  /** Load settings from the Tauri backend */
  async load() {
    try {
      this.current = await fetchSettings();
      this.loaded = true;
    } catch (err) {
      console.error('Failed to load settings:', err);
      this.current = { ...DEFAULT_SETTINGS };
      this.loaded = true;
    }
  }

  /** Persist the current settings to the Tauri backend */
  async save() {
    this.saving = true;
    try {
      await saveSettings(this.current);
    } catch (err) {
      console.error('Failed to save settings:', err);
      throw err;
    } finally {
      this.saving = false;
    }
  }

  /** Update a single setting field and optionally save */
  update<K extends keyof Settings>(key: K, value: Settings[K]) {
    this.current = { ...this.current, [key]: value };
  }
}

export const settingsStore = new SettingsStore();
