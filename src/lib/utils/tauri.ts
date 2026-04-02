import { invoke } from '@tauri-apps/api/core';

// ---- Types matching Rust models ----

export interface Transcription {
  id: string;
  text: string;
  duration_ms: number;
  created_at: string;
  word_count: number;
  cancelled?: boolean;
  raw_text?: string;
  llm_applied?: boolean;
}

export type AppStateEnum = 'Idle' | 'Recording' | 'Transcribing' | 'CleaningUp' | 'Pasting';

export interface Settings {
  push_to_talk_shortcut: string;
  push_to_talk_shortcut_alt?: string | null;
  toggle_shortcut: string;
  toggle_shortcut_alt?: string | null;
  cancel_shortcut: string;
  cancel_shortcut_alt?: string | null;
  show_overlay: boolean;
  auto_paste: boolean;
  restore_clipboard: boolean;
  restore_focus_before_paste: boolean;
  model_path: string;
  language: string;
  max_history: number;
  launch_at_login: boolean;
  llm_cleanup_enabled: boolean;
  auto_check_updates: boolean;
}

export interface ModelStatus {
  downloaded: boolean;
  loaded: boolean;
  path: string | null;
  name: string;
  size_bytes: number | null;
}

// ---- Recording commands ----

export function startRecording(): Promise<void> {
  return invoke('start_recording');
}

export function stopRecording(): Promise<void> {
  return invoke('stop_recording');
}

export function cancelRecording(): Promise<void> {
  return invoke('cancel_recording');
}

// ---- Transcription commands ----

export function getTranscriptions(): Promise<Transcription[]> {
  return invoke('get_transcriptions');
}

export function getLastTranscription(): Promise<Transcription | null> {
  return invoke('get_last_transcription');
}

export function deleteTranscription(id: string): Promise<void> {
  return invoke('delete_transcription', { id });
}

export function clearTranscriptions(): Promise<void> {
  return invoke('clear_transcriptions');
}

export function exportTranscriptionsCsv(): Promise<string> {
  return invoke('export_transcriptions_csv');
}

// ---- Settings commands ----

export function getSettings(): Promise<Settings> {
  return invoke('get_settings');
}

export function updateSettings(newSettings: Settings): Promise<void> {
  return invoke('update_settings', { newSettings });
}

// ---- Permission types and commands ----

export interface PermissionStatus {
  /** "authorized", "denied", "not_determined", or "restricted" */
  microphone: string;
  /** AXIsProcessTrusted() result */
  accessibility_api: boolean;
  /** Functional test result (AXUIElement query) */
  accessibility_functional: boolean;
  /** True if API says trusted but functional test fails (needs app restart) */
  needs_restart: boolean;
}

export function checkMicrophonePermission(): Promise<boolean> {
  return invoke('check_microphone_permission');
}

export function checkAccessibilityPermission(): Promise<boolean> {
  return invoke('check_accessibility_permission');
}

export function requestAccessibilityPermission(): Promise<void> {
  return invoke('request_accessibility_permission');
}

export function requestMicrophonePermission(): Promise<boolean> {
  return invoke('request_microphone_permission');
}

export function checkAllPermissions(): Promise<PermissionStatus> {
  return invoke('check_all_permissions');
}

export function openAccessibilitySettings(): Promise<void> {
  return invoke('open_accessibility_settings');
}

export function openMicrophoneSettings(): Promise<void> {
  return invoke('open_microphone_settings');
}

// ---- Setup / onboarding commands ----

export interface AsrBackendInfo {
  backend: string;
  model_available: boolean;
}

export interface SetupResult {
  backend: string;
  microphone_permission: boolean;
  accessibility_permission: boolean;
  asr_ready: boolean;
  model_available: boolean;
}

export function getAsrBackend(): Promise<AsrBackendInfo> {
  return invoke('get_asr_backend');
}

export function getModelStatus(): Promise<ModelStatus> {
  return invoke('get_model_status');
}

export function needsOnboarding(): Promise<boolean> {
  return invoke('needs_onboarding');
}

export function initAsr(): Promise<void> {
  return invoke('init_asr');
}

export function downloadModel(): Promise<void> {
  return invoke('download_model');
}

export function completeSetup(): Promise<SetupResult> {
  return invoke('complete_setup');
}

// ---- LLM transcript cleanup commands ----

export interface LlmStatus {
  available: boolean;
  unavailable_reason: string | null;
  downloaded: boolean;
  downloading: boolean;
  loaded: boolean;
  model_name: string;
  model_path: string | null;
  update_available: boolean;
}

export function getLlmStatus(): Promise<LlmStatus> {
  return invoke('get_llm_status');
}

export function checkLlmUpdate(): Promise<boolean> {
  return invoke('check_llm_update');
}

export function downloadLlmModel(): Promise<void> {
  return invoke('download_llm_model');
}

export function updateLlmModel(): Promise<void> {
  return invoke('update_llm_model');
}

export function cancelLlmDownload(): Promise<void> {
  return invoke('cancel_llm_download');
}

export function deleteLlmModel(): Promise<void> {
  return invoke('delete_llm_model');
}

export function loadLlmModel(): Promise<void> {
  return invoke('load_llm_model');
}

export function unloadLlmModel(): Promise<void> {
  return invoke('unload_llm_model');
}

// ---- App update commands ----

export interface UpdateStatus {
  update_available: boolean;
  version: string | null;
  release_notes: string | null;
  downloading: boolean;
  restart_pending: boolean;
}

export function checkAppUpdate(): Promise<string | null> {
  return invoke('check_app_update');
}

export function performAppUpdate(): Promise<string> {
  return invoke('perform_app_update');
}

export function getUpdateStatus(): Promise<UpdateStatus> {
  return invoke('get_update_status');
}
