import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { flushSync, mount, unmount } from 'svelte';
const { listeners } = vi.hoisted(() => ({ listeners:new Map<string, (event:{payload:unknown})=>void>() }));
vi.mock('@tauri-apps/api/window', () => ({ getCurrentWindow:() => ({ close:vi.fn(async () => undefined) }) }));
vi.mock('@tauri-apps/api/event', () => ({ listen:vi.fn(async (name:string, callback:(event:{payload:unknown})=>void) => { listeners.set(name, callback); return () => listeners.delete(name); }) }));
vi.mock('../utils/tauri', () => ({
  checkAllPermissions:vi.fn(), completeSetup:vi.fn(), downloadModel:vi.fn(), getAsrBackend:vi.fn(), getSettings:vi.fn(),
  openAccessibilitySettings:vi.fn(), openMicrophoneSettings:vi.fn(), requestAccessibilityPermission:vi.fn(),
  requestMicrophonePermission:vi.fn(), restartApp:vi.fn(),
}));
import { checkAllPermissions, completeSetup, downloadModel, getAsrBackend, getSettings, openAccessibilitySettings, requestAccessibilityPermission } from '../utils/tauri';
import type { SetupResult } from '../utils/tauri';
import { createDefaultSettings } from '../stores/settings.svelte';
import OnboardingView from './onboarding-view.svelte';
const failed:SetupResult = { backend:'FluidAudio', microphone_permission:true, accessibility_permission:true, asr_ready:false, model_available:false, error:'CoreML compile failed: not enough disk space' };
let component:ReturnType<typeof mount>|undefined;
let target:HTMLDivElement;
beforeEach(() => {
  vi.clearAllMocks(); listeners.clear();
  vi.mocked(getAsrBackend).mockResolvedValue({ backend:'FluidAudio', model_available:false });
  vi.mocked(checkAllPermissions).mockResolvedValue({ microphone:'authorized', accessibility_api:false, accessibility_functional:false, needs_restart:false });
  vi.mocked(downloadModel).mockResolvedValue(undefined);
  vi.mocked(requestAccessibilityPermission).mockResolvedValue(undefined);
  vi.mocked(openAccessibilitySettings).mockResolvedValue(undefined);
});
afterEach(async () => { if (component) await unmount(component); component = undefined; target?.remove(); });
const button = (label:string) => Array.from(target.querySelectorAll('button')).find((item) => item.textContent?.trim() === label)!;
async function openPermissions() {
  target = document.createElement('div'); document.body.append(target); component = mount(OnboardingView, { target }); flushSync();
  await vi.waitFor(() => expect(target.textContent).toContain('FluidAudio'));
  button('Get Started').click(); flushSync();
  await vi.waitFor(() => expect(checkAllPermissions).toHaveBeenCalled());
}

it('shows the error setup returns and retries in place', async () => {
  vi.mocked(completeSetup).mockResolvedValueOnce(failed).mockResolvedValueOnce({ ...failed, asr_ready:true, model_available:true, error:null });
  vi.mocked(getSettings).mockResolvedValue({ ...createDefaultSettings(), toggle_shortcut:'Control+Alt+KeyR' });
  await openPermissions();
  button('Continue').click();
  await vi.waitFor(() => expect(target.textContent).toContain('Setup Failed'));
  expect(target.textContent).toContain('CoreML compile failed: not enough disk space');
  expect(downloadModel).toHaveBeenCalledOnce();
  button('Try Again').click();
  await vi.waitFor(() => expect(target.textContent).toContain('SottoASR is Ready!'));
  expect(completeSetup).toHaveBeenCalledTimes(2);
  await vi.waitFor(() => expect(Array.from(target.querySelectorAll('.shortcut')).map((row) => Array.from(row.querySelectorAll('kbd')).map((key) => key.textContent).join(''))).toEqual(['⇧⌘␣', '⌃⌥R', '⎋']));
});

it('skips the download when the model is present and shows download progress while setup runs', async () => {
  vi.mocked(getAsrBackend).mockResolvedValueOnce({ backend:'FluidAudio', model_available:false }).mockResolvedValueOnce({ backend:'FluidAudio', model_available:true });
  let finish!:(result:SetupResult)=>void;
  vi.mocked(completeSetup).mockReturnValueOnce(new Promise((resolve) => { finish = resolve; }));
  await openPermissions();
  button('Continue').click();
  await vi.waitFor(() => expect(completeSetup).toHaveBeenCalledOnce());
  expect(downloadModel).not.toHaveBeenCalled();
  listeners.get('model-download-progress')!({ payload:{ downloaded_bytes:250_000_000, total_bytes:500_000_000 } }); flushSync();
  expect(target.querySelector('[role="progressbar"]')?.getAttribute('aria-valuenow')).toBe('50');
  finish(failed);
  await vi.waitFor(() => expect(target.textContent).toContain('not enough disk space'));
});

it('asks macOS for Accessibility access before opening System Settings', async () => {
  await openPermissions();
  await vi.waitFor(() => expect(button('Grant Access')).toBeDefined());
  button('Grant Access').click();
  await vi.waitFor(() => expect(requestAccessibilityPermission).toHaveBeenCalledOnce());
  await vi.waitFor(() => expect(button('Open System Settings')).toBeDefined());
  button('Open System Settings').click();
  await vi.waitFor(() => expect(openAccessibilitySettings).toHaveBeenCalledOnce());
});
