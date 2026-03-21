import { invoke } from '@tauri-apps/api/core';

// ---- Types matching Rust models ----

export interface Transcription {
  id: string;
  text: string;
  duration_ms: number;
  created_at: string;
  word_count: number;
}

export type AppStateEnum = 'Idle' | 'Recording' | 'Transcribing' | 'Pasting';

export interface Settings {
  push_to_talk_shortcut: string;
  toggle_shortcut: string;
  cancel_shortcut: string;
  show_overlay: boolean;
  auto_paste: boolean;
  restore_clipboard: boolean;
  model_path: string;
  language: string;
  max_history: number;
  launch_at_login: boolean;
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

// ---- Settings commands ----

export function getSettings(): Promise<Settings> {
  return invoke('get_settings');
}

export function updateSettings(newSettings: Settings): Promise<void> {
  return invoke('update_settings', { newSettings });
}

// ---- Permission commands ----

export function checkMicrophonePermission(): Promise<boolean> {
  return invoke('check_microphone_permission');
}

export function checkAccessibilityPermission(): Promise<boolean> {
  return invoke('check_accessibility_permission');
}

export function requestAccessibilityPermission(): Promise<void> {
  return invoke('request_accessibility_permission');
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
