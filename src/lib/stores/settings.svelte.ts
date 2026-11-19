import { getSettings, getSettingsLoadError, resetSettings, updateSettings } from '../utils/tauri';
import type { DictionaryEntry, Settings, UpdateSettingsResult } from '../utils/tauri';

export function createDefaultSettings(): Settings {
  return {
    push_to_talk_shortcut: 'CommandOrControl+Shift+Space', push_to_talk_shortcut_alt: null,
    toggle_shortcut: 'CommandOrControl+Shift+D', toggle_shortcut_alt: null,
    cancel_shortcut: 'Escape', cancel_shortcut_alt: null,
    open_settings_shortcut: 'CommandOrControl+Shift+Comma',
    show_overlay: true, auto_paste: true, restore_clipboard: true, restore_focus_before_paste: true,
    launch_at_login: false,
    llm_cleanup_enabled: false, llm_cleanup_mode: 'retype', dictionary: [], vocabulary: [], auto_check_updates: true,
    input_device: null, sound_cues: false, history_retention_days: 0, audio_retention_days: 0,
  };
}

/** Shown when Save is blocked by a replacement row with only one field filled in. */
export const INCOMPLETE_REPLACEMENT_MESSAGE = 'Finish or remove the incomplete replacement in Vocabulary before saving.';

/** One field filled in and the other empty. Rows with both fields empty are dropped on save. */
export function isIncompleteEntry(entry: DictionaryEntry): boolean {
  return !entry.heard.trim() !== !entry.replacement.trim();
}

/**
 * The backend omits empty optional fields, so every copy of the settings is
 * filled from the defaults. Equal settings then serialize equally, and a
 * no-op edit is not reported as unsaved.
 */
function withDefaults(settings: Settings): Settings {
  return { ...createDefaultSettings(), ...settings, dictionary: settings.dictionary ?? [], vocabulary: settings.vocabulary ?? [] };
}

const snapshot = (settings: Settings): Settings => JSON.parse(JSON.stringify(settings));
const message = (error: unknown) => error instanceof Error ? error.message : String(error);

export class SettingsStore {
  current: Settings = $state(createDefaultSettings());
  saved: Settings | null = $state(null);
  loaded = $state(false);
  loading = $state(false);
  saving = $state(false);
  resetting = $state(false);
  error = $state('');
  /** Why the settings file could not be read; set only when "Back up and reset" can repair it. */
  loadError: string | null = $state(null);
  warnings: string[] = $state([]);
  private loadGeneration = 0;
  private pendingSave: Promise<UpdateSettingsResult> | null = null;

  get dirty() {
    return this.loaded && this.saved !== null && JSON.stringify(this.current) !== JSON.stringify(this.saved);
  }

  /** A problem in the draft that blocks Save; empty when the draft can be saved. */
  get validationError() {
    return this.current.dictionary.some(isIncompleteEntry) ? INCOMPLETE_REPLACEMENT_MESSAGE : '';
  }

  private adopt(settings: Settings) {
    this.current = withDefaults(settings);
    this.saved = snapshot(this.current);
    this.loaded = true;
    this.loadError = null;
    this.warnings = [];
  }

  load(): Promise<boolean> {
    return this.fetch(++this.loadGeneration);
  }

  private async fetch(generation: number): Promise<boolean> {
    this.loading = true;
    this.loaded = false;
    this.error = '';
    this.loadError = null;
    try {
      const fetched = await getSettings();
      if (generation !== this.loadGeneration) return false;
      this.adopt(fetched);
      return true;
    } catch (error) {
      if (generation !== this.loadGeneration) return false;
      this.error = `Could not load settings: ${message(error)}`;
      const reason = await getSettingsLoadError().catch(() => null);
      if (generation === this.loadGeneration) this.loadError = reason;
      return false;
    } finally {
      if (generation === this.loadGeneration) this.loading = false;
    }
  }

  /** Back up the unreadable settings file and adopt the defaults the backend wrote. */
  async reset(): Promise<boolean> {
    if (this.resetting) return false;
    const generation = ++this.loadGeneration;
    this.resetting = true;
    this.error = '';
    try {
      const fetched = await resetSettings();
      if (generation !== this.loadGeneration) return false;
      this.adopt(fetched);
      return true;
    } catch (error) {
      if (generation !== this.loadGeneration) return false;
      // A reset can fail after it wrote the defaults, for example when a
      // default shortcut is taken by another app. Reload, so those defaults
      // appear with the backend's explanation instead of a second reset offer.
      const reason = message(error);
      const reloaded = await this.fetch(generation);
      if (generation === this.loadGeneration) this.error = reloaded ? reason : `Could not reset settings: ${reason}`;
      return false;
    } finally {
      this.resetting = false;
    }
  }

  save(): Promise<UpdateSettingsResult> {
    if (this.pendingSave) return this.pendingSave;
    if (!this.loaded || this.loading || !this.saved) return Promise.reject(new Error('Load settings before saving'));
    const problem = this.validationError;
    if (problem) return Promise.reject(new Error(problem));
    const draftJson = JSON.stringify(this.current);
    const submitted = snapshot(this.current);
    submitted.dictionary = submitted.dictionary
      .map((entry) => ({ heard: entry.heard.trim(), replacement: entry.replacement.trim() }))
      .filter((entry) => entry.heard || entry.replacement);
    const generation = this.loadGeneration;
    this.saving = true;
    this.error = '';
    this.pendingSave = updateSettings(submitted).then((result) => {
      if (generation === this.loadGeneration) {
        const saved = withDefaults(result.settings);
        // Only the submitted version can be acknowledged as saved. Later edits
        // remain a draft even if the backend normalizes the submitted values.
        if (JSON.stringify(this.current) === draftJson) this.current = snapshot(saved);
        this.saved = snapshot(saved);
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
