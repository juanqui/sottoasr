import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { flushSync, mount, unmount } from 'svelte';
const { listeners, onClose, closeWindow } = vi.hoisted(() => ({
  listeners:new Map<string, (event:unknown)=>void>(),
  onClose:vi.fn(async (_handler:(event:{preventDefault:()=>void})=>void) => vi.fn()),
  closeWindow:vi.fn(async () => undefined),
}));
vi.mock('@tauri-apps/api/window', () => ({ getCurrentWindow: () => ({ onCloseRequested:onClose, close:closeWindow }) }));
vi.mock('@tauri-apps/api/event', () => ({ listen:vi.fn(async (name:string, callback:(event:unknown)=>void) => { listeners.set(name,callback); return () => listeners.delete(name); }) }));
vi.mock('../utils/tauri', async (importOriginal) => ({ ...await importOriginal<typeof import('../utils/tauri')>(), getSettings:vi.fn(), updateSettings:vi.fn(), getLlmStatus:vi.fn(), prepareLlmModel:vi.fn(), getVocabularyStatus:vi.fn(), getModelStatus:vi.fn() }));
import SettingsPanel from './settings-panel.svelte';
import { listen } from '@tauri-apps/api/event';
import { settingsStore, createDefaultSettings } from '../stores/settings.svelte';
import { getSettings, updateSettings, getLlmStatus, prepareLlmModel, getVocabularyStatus, getModelStatus } from '../utils/tauri';
import type { LlmStatus } from '../utils/tauri';
const status:LlmStatus = { available:true, unavailable_reason:null, downloaded:false, downloading:false, loaded:false, preparing:false, setup_error:null, update_available:false, model_name:'Test cleanup', model_path:null, model_url:'https://example.com/model', download_size_mb:227, last_cleanup_status:{kind:'idle'} };
let component:ReturnType<typeof mount>|undefined;
let target:HTMLDivElement;
beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(listen).mockImplementation(async (name, callback) => { listeners.set(name, callback as (event:unknown)=>void); return () => { listeners.delete(name); }; });
  vi.mocked(getModelStatus).mockResolvedValue({loaded:true,downloaded:true,path:null,name:'Parakeet',size_bytes:null});
  vi.mocked(getSettings).mockResolvedValue(createDefaultSettings());
  vi.mocked(updateSettings).mockImplementation(async (settings) => ({ settings, warnings:[] }));
  vi.mocked(getLlmStatus).mockResolvedValue({...status});
  vi.mocked(getVocabularyStatus).mockResolvedValue({supported:true, downloaded:false, loaded:false, preparing:false, download_size_mb:300, error:null});
  settingsStore.loaded = false;
  listeners.clear();
  HTMLDialogElement.prototype.showModal = function () { this.open = true; };
  HTMLDialogElement.prototype.close = function () { this.open = false; };
});
afterEach(async () => { if (component) await unmount(component); component = undefined; target?.remove(); });
async function render() {
  target = document.createElement('div'); document.body.append(target); component = mount(SettingsPanel, {target}); flushSync();
  await vi.waitFor(() => expect(settingsStore.loading).toBe(false)); flushSync();
}
const button = (label:string) => Array.from(target.querySelectorAll('button')).find((item) => item.textContent?.trim() === label)!;
const input = (label:string) => target.querySelector<HTMLInputElement>(`input[aria-label="${label}"]`)!;
function type(element:HTMLInputElement, value:string) { element.value = value; element.dispatchEvent(new Event('input',{bubbles:true})); flushSync(); }
function cleanupToggle() { return Array.from(target.querySelectorAll('label')).find((label) => label.textContent?.includes('AI transcript cleanup'))!.querySelector<HTMLInputElement>('input')!; }

it('starts setup from the off toggle and does not enable after failure', async () => {
  vi.mocked(prepareLlmModel).mockRejectedValueOnce(new Error('Network unavailable'));
  await render(); button('Dictation').click(); flushSync();
  expect(cleanupToggle().disabled).toBe(false); cleanupToggle().click();
  await vi.waitFor(() => expect(target.textContent).toContain('Network unavailable'));
  expect(settingsStore.current.llm_cleanup_enabled).toBe(false); expect(updateSettings).not.toHaveBeenCalled();
  vi.mocked(prepareLlmModel).mockResolvedValueOnce({...status, downloaded:true, loaded:true});
  button('Retry setup').click(); await vi.waitFor(() => expect(settingsStore.current.llm_cleanup_enabled).toBe(true));
  expect(settingsStore.saved?.llm_cleanup_enabled).toBe(false);
  button('Save').click(); await vi.waitFor(() => expect(settingsStore.saved?.llm_cleanup_enabled).toBe(true));
  await vi.waitFor(() => expect(target.textContent).not.toContain('Ready. Save to enable AI cleanup.'));
});

it('shares the draft across sections and cancels vocabulary/replacement edits', async () => {
  await render(); button('Vocabulary').click(); flushSync();
  type(target.querySelector<HTMLInputElement>('input[placeholder^="Qwen"]')!, 'Qwen'); button('Add').click(); flushSync();
  button('Add replacement').click(); flushSync(); type(input('Heard alias 1'),'Quen'); type(input('Replacement 1'),'Qwen');
  button('Dictation').click(); flushSync(); button('Vocabulary').click(); flushSync();
  expect(settingsStore.current.vocabulary).toEqual(['Qwen']); expect(input('Replacement 1').value).toBe('Qwen');
  button('Cancel').click(); flushSync(); expect(settingsStore.current.vocabulary).toEqual([]); expect(settingsStore.current.dictionary).toEqual([]);
  expect(target.querySelector('button button')).toBeNull();
});

it('blocks editing after load failure and supports retry', async () => {
  vi.mocked(getSettings).mockRejectedValueOnce(new Error('Unreadable settings'));
  await render(); expect(target.textContent).toContain('Unreadable settings'); expect(button('Save').disabled).toBe(true);
  expect(target.querySelector('.footer-feedback')?.textContent).toContain('Settings unavailable');
  expect(target.querySelector('input')).toBeNull(); button('Retry').click(); await vi.waitFor(() => expect(settingsStore.loaded).toBe(true));
});

it('supports arrow-key navigation with a single selected tab', async () => {
  await render(); button('General').dispatchEvent(new KeyboardEvent('keydown',{key:'ArrowRight', bubbles:true})); flushSync();
  expect(button('Dictation').getAttribute('aria-selected')).toBe('true'); expect(document.activeElement).toBe(button('Dictation'));
  expect(target.querySelectorAll('[role="tab"][tabindex="0"]')).toHaveLength(1);
  for (const tab of target.querySelectorAll('[role="tab"]')) expect(document.getElementById(tab.getAttribute('aria-controls')!)).not.toBeNull();
});


it('saves unrelated edits during preparation and leaves late activation unsaved', async () => {
  let ready!: (status:LlmStatus)=>void;
  vi.mocked(prepareLlmModel).mockReturnValueOnce(new Promise((resolve) => { ready=resolve; }));
  await render(); button('Dictation').click(); flushSync(); cleanupToggle().click(); flushSync();
  settingsStore.update('show_overlay',false); flushSync();
  expect(button('Save').disabled).toBe(false); button('Save').click();
  await vi.waitFor(() => expect(updateSettings).toHaveBeenCalledOnce());
  expect(vi.mocked(updateSettings).mock.calls[0][0].llm_cleanup_enabled).toBe(false);
  expect(settingsStore.saved?.show_overlay).toBe(false);
  ready({...status,downloaded:true,loaded:true});
  await vi.waitFor(() => expect(settingsStore.current.llm_cleanup_enabled).toBe(true));
  expect(settingsStore.saved?.llm_cleanup_enabled).toBe(false); expect(settingsStore.dirty).toBe(true);
  flushSync();
  expect(target.textContent).toContain('Ready. Save to enable AI cleanup.');
});

it('subscribes before its first cleanup status read so setup completion cannot fall in a gap', async () => {
  let subscribed!: (off:()=>void)=>void;
  vi.mocked(listen).mockImplementation((name) => name === 'llm-preparation-changed' ? new Promise((resolve) => {subscribed=resolve;}) : Promise.resolve(vi.fn()));
  await render(); expect(getLlmStatus).not.toHaveBeenCalled();
  vi.mocked(getLlmStatus).mockResolvedValue({...status,downloaded:true,loaded:true});
  subscribed(vi.fn()); await vi.waitFor(() => expect(getLlmStatus).toHaveBeenCalledOnce());
  button('Dictation').click(); flushSync();
  await vi.waitFor(() => expect(target.textContent).not.toContain('one-time 227 MB'));
});

it('allows the native X close immediately when the Settings draft is unchanged', async () => {
  await render();
  const event = { preventDefault: vi.fn() };
  onClose.mock.calls[0][0](event); flushSync();
  expect(event.preventDefault).not.toHaveBeenCalled();
  expect(target.querySelector('dialog')?.open).toBe(false);
});

it.each(['Discard', 'Save and close'])('protects edits on X and then permits %s to close', async (action) => {
  await render(); settingsStore.update('show_overlay', false); flushSync();
  const request = onClose.mock.calls[0][0];
  const event = { preventDefault: vi.fn() };
  request(event); flushSync();
  expect(event.preventDefault).toHaveBeenCalledOnce();
  expect(target.querySelector('dialog')?.open).toBe(true);
  button('Keep editing').click(); flushSync();
  expect(closeWindow).not.toHaveBeenCalled();
  expect(settingsStore.dirty).toBe(true);
  request({ preventDefault:vi.fn() }); flushSync(); button(action).click();
  await vi.waitFor(() => expect(closeWindow).toHaveBeenCalledOnce());
  expect(settingsStore.dirty).toBe(false);
  const permitted = { preventDefault: vi.fn() };
  request(permitted); expect(permitted.preventDefault).not.toHaveBeenCalled();
});

it('keeps Settings open with its draft intact when Save and close cannot persist', async () => {
  await render(); settingsStore.update('show_overlay', false); flushSync();
  vi.mocked(updateSettings).mockRejectedValueOnce(new Error('Disk full'));
  onClose.mock.calls[0][0]({ preventDefault:vi.fn() }); flushSync(); button('Save and close').click();
  await vi.waitFor(() => expect(settingsStore.error).toContain('Disk full')); flushSync();
  expect(closeWindow).not.toHaveBeenCalled();
  expect(settingsStore.dirty).toBe(true);
  expect(settingsStore.current.show_overlay).toBe(false);
  expect(target.textContent).toContain('Disk full');
});

it('retains the setup Save reminder after a failed enable preference save', async () => {
  vi.mocked(prepareLlmModel).mockResolvedValueOnce({...status, downloaded:true, loaded:true});
  await render(); button('Dictation').click(); flushSync(); cleanupToggle().click();
  await vi.waitFor(() => expect(target.textContent).toContain('Ready. Save to enable AI cleanup.'));
  vi.mocked(updateSettings).mockRejectedValueOnce(new Error('Disk full'));
  button('Save').click(); await vi.waitFor(() => expect(settingsStore.error).toContain('Disk full')); flushSync();
  expect(target.textContent).toContain('Ready. Save to enable AI cleanup.');
  expect(settingsStore.saved?.llm_cleanup_enabled).toBe(false);
});
