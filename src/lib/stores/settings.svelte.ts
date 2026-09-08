import { getSettings, updateSettings } from '../utils/tauri';
import type { Settings, UpdateSettingsResult } from '../utils/tauri';

export function createDefaultSettings(): Settings {
  return {
    push_to_talk_shortcut: 'CommandOrControl+Shift+Space', push_to_talk_shortcut_alt: null,
    toggle_shortcut: 'CommandOrControl+Shift+D', toggle_shortcut_alt: null,
    cancel_shortcut: 'Escape', cancel_shortcut_alt: null,
    open_settings_shortcut: 'CommandOrControl+Shift+Comma',
    show_overlay: true, auto_paste: true, restore_clipboard: true, restore_focus_before_paste: true,
    model_path: '', language: 'auto', max_history: 500, launch_at_login: false,
    llm_cleanup_enabled: false, dictionary: [], vocabulary: [], auto_check_updates: true,
  };
}

const snapshot = (settings: Settings): Settings => JSON.parse(JSON.stringify(settings));
const message = (error: unknown) => error instanceof Error ? error.message : String(error);

export class SettingsStore {
  current: Settings = $state(createDefaultSettings());
  saved: Settings | null = $state(null);
  loaded = $state(false);
  loading = $state(false);
  saving = $state(false);
  error = $state('');
  warnings: string[] = $state([]);
  private loadGeneration = 0;
  private pendingSave: Promise<UpdateSettingsResult> | null = null;

  get dirty() {
    return this.loaded && this.saved !== null && JSON.stringify(this.current) !== JSON.stringify(this.saved);
  }

  async load(): Promise<boolean> {
    const generation = ++this.loadGeneration;
    this.loading = true;
    this.loaded = false;
    this.error = '';
    try {
      const fetched = await getSettings();
      if (generation !== this.loadGeneration) return false;
      this.current = { ...createDefaultSettings(), ...fetched, dictionary: fetched.dictionary ?? [], vocabulary: fetched.vocabulary ?? [] };
      this.saved = snapshot(this.current);
      this.loaded = true;
      this.warnings = [];
      return true;
    } catch (error) {
      if (generation === this.loadGeneration) this.error = `Could not load settings: ${message(error)}`;
      return false;
    } finally {
      if (generation === this.loadGeneration) this.loading = false;
    }
  }

  save(): Promise<UpdateSettingsResult> {
    if (this.pendingSave) return this.pendingSave;
    if (!this.loaded || this.loading || !this.saved) return Promise.reject(new Error('Load settings before saving'));
    const submitted = snapshot(this.current);
    const submittedJson = JSON.stringify(submitted);
    const generation = this.loadGeneration;
    this.saving = true;
    this.error = '';
    this.pendingSave = updateSettings(submitted).then((result) => {
      if (generation === this.loadGeneration) {
        // Only the submitted version can be acknowledged as saved. Later edits
        // remain a draft even if the backend normalizes the submitted values.
        if (JSON.stringify(this.current) === submittedJson) this.current = snapshot(result.settings);
        this.saved = snapshot(result.settings);
        this.warnings = result.warnings;
      }
      return result;
    }).catch((error) => {
      if (generation === this.loadGeneration) this.error = `Could not save settings: ${message(error)}`;
      throw error;
    }).finally(() => {
      this.saving = false;
      this.pendingSave = null;
    });
    return this.pendingSave;
  }

  invalidateLoad() { ++this.loadGeneration; }

  discard() {
    if (this.saved) this.current = snapshot(this.saved);
    this.error = '';
    this.warnings = [];
  }

  update<K extends keyof Settings>(key: K, value: Settings[K]) {
    this.current = { ...this.current, [key]: value };
  }
}

export const settingsStore = new SettingsStore();
