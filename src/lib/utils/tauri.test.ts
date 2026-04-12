import { describe, it, expect, vi, beforeEach } from 'vitest';

// Mock @tauri-apps/api/core before importing the module under test
vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(),
}));

import { invoke } from '@tauri-apps/api/core';

import {
  // Recording
  startRecording,
  stopRecording,
  cancelRecording,
  // Transcription
  getTranscriptions,
  getLastTranscription,
  deleteTranscription,
  clearTranscriptions,
  exportTranscriptionsCsv,
  // Settings
  getSettings,
  updateSettings,
  // Permissions
  checkMicrophonePermission,
  checkAccessibilityPermission,
  requestAccessibilityPermission,
  requestMicrophonePermission,
  checkAllPermissions,
  openAccessibilitySettings,
  openMicrophoneSettings,
  // Setup / onboarding
  getAsrBackend,
  getModelStatus,
  needsOnboarding,
  initAsr,
  downloadModel,
  completeSetup,
  // LLM
  getLlmStatus,
  checkLlmUpdate,
  downloadLlmModel,
  updateLlmModel,
  cancelLlmDownload,
  deleteLlmModel,
  loadLlmModel,
  unloadLlmModel,
  // Updater
  checkAppUpdate,
  performAppUpdate,
  getUpdateStatus,
} from './tauri';

import type { Settings } from './tauri';

const mockInvoke = vi.mocked(invoke);

beforeEach(() => {
  mockInvoke.mockReset();
});

// ---------------------------------------------------------------------------
// Recording commands
// ---------------------------------------------------------------------------
describe('Recording commands', () => {
  it('startRecording invokes start_recording', async () => {
    mockInvoke.mockResolvedValueOnce(undefined);
    await startRecording();
    expect(mockInvoke).toHaveBeenCalledWith('start_recording');
  });

  it('stopRecording invokes stop_recording', async () => {
    mockInvoke.mockResolvedValueOnce(undefined);
    await stopRecording();
    expect(mockInvoke).toHaveBeenCalledWith('stop_recording');
  });

  it('cancelRecording invokes cancel_recording', async () => {
    mockInvoke.mockResolvedValueOnce(undefined);
    await cancelRecording();
    expect(mockInvoke).toHaveBeenCalledWith('cancel_recording');
  });
});

// ---------------------------------------------------------------------------
// Transcription commands
// ---------------------------------------------------------------------------
describe('Transcription commands', () => {
  it('getTranscriptions invokes get_transcriptions', async () => {
    mockInvoke.mockResolvedValueOnce([]);
    const result = await getTranscriptions();
    expect(mockInvoke).toHaveBeenCalledWith('get_transcriptions');
    expect(result).toEqual([]);
  });

  it('getLastTranscription invokes get_last_transcription', async () => {
    mockInvoke.mockResolvedValueOnce(null);
    const result = await getLastTranscription();
    expect(mockInvoke).toHaveBeenCalledWith('get_last_transcription');
    expect(result).toBeNull();
  });

  it('deleteTranscription invokes delete_transcription with id', async () => {
    mockInvoke.mockResolvedValueOnce(undefined);
    await deleteTranscription('abc-123');
    expect(mockInvoke).toHaveBeenCalledWith('delete_transcription', { id: 'abc-123' });
  });

  it('clearTranscriptions invokes clear_transcriptions', async () => {
    mockInvoke.mockResolvedValueOnce(undefined);
    await clearTranscriptions();
    expect(mockInvoke).toHaveBeenCalledWith('clear_transcriptions');
  });

  it('exportTranscriptionsCsv invokes export_transcriptions_csv', async () => {
    mockInvoke.mockResolvedValueOnce('csv-data');
    const result = await exportTranscriptionsCsv();
    expect(mockInvoke).toHaveBeenCalledWith('export_transcriptions_csv');
    expect(result).toBe('csv-data');
  });
});

// ---------------------------------------------------------------------------
// Settings commands
// ---------------------------------------------------------------------------
describe('Settings commands', () => {
  const mockSettings: Settings = {
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

  it('getSettings invokes get_settings', async () => {
    mockInvoke.mockResolvedValueOnce(mockSettings);
    const result = await getSettings();
    expect(mockInvoke).toHaveBeenCalledWith('get_settings');
    expect(result).toEqual(mockSettings);
  });

  it('updateSettings invokes update_settings with newSettings', async () => {
    mockInvoke.mockResolvedValueOnce(undefined);
    await updateSettings(mockSettings);
    expect(mockInvoke).toHaveBeenCalledWith('update_settings', { newSettings: mockSettings });
  });
});

// ---------------------------------------------------------------------------
// Permission commands
// ---------------------------------------------------------------------------
describe('Permission commands', () => {
  it('checkMicrophonePermission invokes check_microphone_permission', async () => {
    mockInvoke.mockResolvedValueOnce(true);
    const result = await checkMicrophonePermission();
    expect(mockInvoke).toHaveBeenCalledWith('check_microphone_permission');
    expect(result).toBe(true);
  });

  it('checkAccessibilityPermission invokes check_accessibility_permission', async () => {
    mockInvoke.mockResolvedValueOnce(false);
    const result = await checkAccessibilityPermission();
    expect(mockInvoke).toHaveBeenCalledWith('check_accessibility_permission');
    expect(result).toBe(false);
  });

  it('requestAccessibilityPermission invokes request_accessibility_permission', async () => {
    mockInvoke.mockResolvedValueOnce(undefined);
    await requestAccessibilityPermission();
    expect(mockInvoke).toHaveBeenCalledWith('request_accessibility_permission');
  });

  it('requestMicrophonePermission invokes request_microphone_permission', async () => {
    mockInvoke.mockResolvedValueOnce(true);
    const result = await requestMicrophonePermission();
    expect(mockInvoke).toHaveBeenCalledWith('request_microphone_permission');
    expect(result).toBe(true);
  });

  it('checkAllPermissions invokes check_all_permissions', async () => {
    const status = {
      microphone: 'authorized',
      accessibility_api: true,
      accessibility_functional: true,
      needs_restart: false,
    };
    mockInvoke.mockResolvedValueOnce(status);
    const result = await checkAllPermissions();
    expect(mockInvoke).toHaveBeenCalledWith('check_all_permissions');
    expect(result).toEqual(status);
  });

  it('openAccessibilitySettings invokes open_accessibility_settings', async () => {
    mockInvoke.mockResolvedValueOnce(undefined);
    await openAccessibilitySettings();
    expect(mockInvoke).toHaveBeenCalledWith('open_accessibility_settings');
  });

  it('openMicrophoneSettings invokes open_microphone_settings', async () => {
    mockInvoke.mockResolvedValueOnce(undefined);
    await openMicrophoneSettings();
    expect(mockInvoke).toHaveBeenCalledWith('open_microphone_settings');
  });
});

// ---------------------------------------------------------------------------
// Setup / onboarding commands
// ---------------------------------------------------------------------------
describe('Setup / onboarding commands', () => {
  it('getAsrBackend invokes get_asr_backend', async () => {
    const info = { backend: 'fluidaudio', model_available: true };
    mockInvoke.mockResolvedValueOnce(info);
    const result = await getAsrBackend();
    expect(mockInvoke).toHaveBeenCalledWith('get_asr_backend');
    expect(result).toEqual(info);
  });

  it('getModelStatus invokes get_model_status', async () => {
    const status = { downloaded: true, loaded: true, path: '/models/whisper', name: 'whisper', size_bytes: 500_000_000 };
    mockInvoke.mockResolvedValueOnce(status);
    const result = await getModelStatus();
    expect(mockInvoke).toHaveBeenCalledWith('get_model_status');
    expect(result).toEqual(status);
  });

  it('needsOnboarding invokes needs_onboarding', async () => {
    mockInvoke.mockResolvedValueOnce(false);
    const result = await needsOnboarding();
    expect(mockInvoke).toHaveBeenCalledWith('needs_onboarding');
    expect(result).toBe(false);
  });

  it('initAsr invokes init_asr', async () => {
    mockInvoke.mockResolvedValueOnce(undefined);
    await initAsr();
    expect(mockInvoke).toHaveBeenCalledWith('init_asr');
  });

  it('downloadModel invokes download_model', async () => {
    mockInvoke.mockResolvedValueOnce(undefined);
    await downloadModel();
    expect(mockInvoke).toHaveBeenCalledWith('download_model');
  });

  it('completeSetup invokes complete_setup', async () => {
    const result = {
      backend: 'fluidaudio',
      microphone_permission: true,
      accessibility_permission: true,
      asr_ready: true,
      model_available: true,
    };
    mockInvoke.mockResolvedValueOnce(result);
    const setupResult = await completeSetup();
    expect(mockInvoke).toHaveBeenCalledWith('complete_setup');
    expect(setupResult).toEqual(result);
  });
});

// ---------------------------------------------------------------------------
// LLM commands
// ---------------------------------------------------------------------------
describe('LLM commands', () => {
  it('getLlmStatus invokes get_llm_status', async () => {
    const status = {
      available: true,
      unavailable_reason: null,
      downloaded: true,
      downloading: false,
      loaded: false,
      model_name: 'sotto-cleanup',
      model_path: '/models/llm',
      update_available: false,
    };
    mockInvoke.mockResolvedValueOnce(status);
    const result = await getLlmStatus();
    expect(mockInvoke).toHaveBeenCalledWith('get_llm_status');
    expect(result).toEqual(status);
  });

  it('checkLlmUpdate invokes check_llm_update', async () => {
    mockInvoke.mockResolvedValueOnce(true);
    const result = await checkLlmUpdate();
    expect(mockInvoke).toHaveBeenCalledWith('check_llm_update');
    expect(result).toBe(true);
  });

  it('downloadLlmModel invokes download_llm_model', async () => {
    mockInvoke.mockResolvedValueOnce(undefined);
    await downloadLlmModel();
    expect(mockInvoke).toHaveBeenCalledWith('download_llm_model');
  });

  it('updateLlmModel invokes update_llm_model', async () => {
    mockInvoke.mockResolvedValueOnce(undefined);
    await updateLlmModel();
    expect(mockInvoke).toHaveBeenCalledWith('update_llm_model');
  });

  it('cancelLlmDownload invokes cancel_llm_download', async () => {
    mockInvoke.mockResolvedValueOnce(undefined);
    await cancelLlmDownload();
    expect(mockInvoke).toHaveBeenCalledWith('cancel_llm_download');
  });

  it('deleteLlmModel invokes delete_llm_model', async () => {
    mockInvoke.mockResolvedValueOnce(undefined);
    await deleteLlmModel();
    expect(mockInvoke).toHaveBeenCalledWith('delete_llm_model');
  });

  it('loadLlmModel invokes load_llm_model', async () => {
    mockInvoke.mockResolvedValueOnce(undefined);
    await loadLlmModel();
    expect(mockInvoke).toHaveBeenCalledWith('load_llm_model');
  });

  it('unloadLlmModel invokes unload_llm_model', async () => {
    mockInvoke.mockResolvedValueOnce(undefined);
    await unloadLlmModel();
    expect(mockInvoke).toHaveBeenCalledWith('unload_llm_model');
  });
});

// ---------------------------------------------------------------------------
// Updater commands
// ---------------------------------------------------------------------------
describe('Updater commands', () => {
  it('checkAppUpdate invokes check_app_update', async () => {
    mockInvoke.mockResolvedValueOnce('0.7.0');
    const result = await checkAppUpdate();
    expect(mockInvoke).toHaveBeenCalledWith('check_app_update');
    expect(result).toBe('0.7.0');
  });

  it('checkAppUpdate returns null when no update available', async () => {
    mockInvoke.mockResolvedValueOnce(null);
    const result = await checkAppUpdate();
    expect(mockInvoke).toHaveBeenCalledWith('check_app_update');
    expect(result).toBeNull();
  });

  it('performAppUpdate invokes perform_app_update', async () => {
    mockInvoke.mockResolvedValueOnce('Updated to 0.7.0');
    const result = await performAppUpdate();
    expect(mockInvoke).toHaveBeenCalledWith('perform_app_update');
    expect(result).toBe('Updated to 0.7.0');
  });

  it('getUpdateStatus invokes get_update_status', async () => {
    const status = {
      update_available: false,
      version: null,
      release_notes: null,
      downloading: false,
      restart_pending: false,
    };
    mockInvoke.mockResolvedValueOnce(status);
    const result = await getUpdateStatus();
    expect(mockInvoke).toHaveBeenCalledWith('get_update_status');
    expect(result).toEqual(status);
  });
});
