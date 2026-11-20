import { getSettings, getSettingsLoadError, resetSettings, updateSettings } from '../utils/tauri';
import type { AwsBedrockSettings, AwsTranscribeSettings, DictionaryEntry, Settings, UpdateSettingsResult } from '../utils/tauri';

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
    speech_engine: 'local', aws_transcribe: { profile: null, region: null, language: 'en-US', vocabulary_name: null },
    cleanup_engine: 'local', aws_bedrock: { profile: null, region: null, model: 'haiku-5.5' },
  };
}

/** Shown when Save is blocked by a replacement row with only one field filled in. */
export const INCOMPLETE_REPLACEMENT_MESSAGE = 'Finish or remove the incomplete replacement in Vocabulary before saving.';

/** Shown when Save is blocked because AWS Transcribe is selected without a profile. */
export const AWS_PROFILE_REQUIRED_MESSAGE = 'Choose an AWS profile for AWS Transcribe.';

/** Shown when Save is blocked because cleanup is on with AWS Bedrock and no profile. */
export const AWS_BEDROCK_PROFILE_REQUIRED_MESSAGE = 'Choose an AWS profile for AWS Bedrock.';

/** One field filled in and the other empty. Rows with both fields empty are dropped on save. */
export function isIncompleteEntry(entry: DictionaryEntry): boolean {
  return !entry.heard.trim() !== !entry.replacement.trim();
}

const optional = (value: string | null | undefined) => value?.trim() || null;

/** The AWS options as the backend accepts them: trimmed, with an empty optional value as null. */
export function normalizedAwsTranscribe(options: AwsTranscribeSettings): AwsTranscribeSettings {
  return { profile: optional(options.profile), region: optional(options.region), language: options.language, vocabulary_name: optional(options.vocabulary_name) };
}

/** The AWS Bedrock options as the backend accepts them: trimmed, with an empty optional value as null. */
export function normalizedAwsBedrock(options: AwsBedrockSettings): AwsBedrockSettings {
  return { profile: optional(options.profile), region: optional(options.region), model: options.model };
}

/**
 * The backend omits empty optional fields, so every copy of the settings is
 * filled from the defaults. Equal settings then serialize equally, and a
 * no-op edit is not reported as unsaved. The AWS options get a fixed key
 * order for the same reason.
 */
function withDefaults(settings: Settings): Settings {
  const aws = settings.aws_transcribe;
  const bedrock = settings.aws_bedrock;
  return {
    ...createDefaultSettings(), ...settings, dictionary: settings.dictionary ?? [], vocabulary: settings.vocabulary ?? [],
    aws_transcribe: { profile: aws?.profile ?? null, region: aws?.region ?? null, language: aws?.language ?? 'en-US', vocabulary_name: aws?.vocabulary_name ?? null },
    aws_bedrock: { profile: bedrock?.profile ?? null, region: bedrock?.region ?? null, model: bedrock?.model ?? 'haiku-5.5' },
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
    if (this.current.dictionary.some(isIncompleteEntry)) return INCOMPLETE_REPLACEMENT_MESSAGE;
    if (this.current.speech_engine === 'aws_transcribe' && !this.current.aws_transcribe.profile?.trim()) return AWS_PROFILE_REQUIRED_MESSAGE;
    if (this.current.llm_cleanup_enabled && this.current.cleanup_engine === 'aws_bedrock' && !this.current.aws_bedrock.profile?.trim()) return AWS_BEDROCK_PROFILE_REQUIRED_MESSAGE;
    return '';
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
    submitted.aws_transcribe = normalizedAwsTranscribe(submitted.aws_transcribe);
    submitted.aws_bedrock = normalizedAwsBedrock(submitted.aws_bedrock);
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
