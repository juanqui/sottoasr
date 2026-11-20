import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { flushSync, mount, unmount } from 'svelte';
const { listeners, onClose, closeWindow } = vi.hoisted(() => ({
  listeners:new Map<string, (event:unknown)=>void>(),
  onClose:vi.fn(async (_handler:(event:{preventDefault:()=>void})=>void) => vi.fn()),
  closeWindow:vi.fn(async () => undefined),
}));
vi.mock('@tauri-apps/api/window', () => ({ getCurrentWindow: () => ({ onCloseRequested:onClose, close:closeWindow }) }));
vi.mock('@tauri-apps/api/event', () => ({ listen:vi.fn(async (name:string, callback:(event:unknown)=>void) => { listeners.set(name,callback); return () => listeners.delete(name); }) }));
vi.mock('../utils/tauri', async (importOriginal) => ({ ...await importOriginal<typeof import('../utils/tauri')>(), getSettings:vi.fn(), updateSettings:vi.fn(), getSettingsLoadError:vi.fn(), resetSettings:vi.fn(), listInputDevices:vi.fn(), checkAllPermissions:vi.fn(), getLlmStatus:vi.fn(), prepareLlmModel:vi.fn(), getVocabularyStatus:vi.fn(), getModelStatus:vi.fn(), getAwsTranscribeOptions:vi.fn(), getBedrockCleanupOptions:vi.fn() }));
import SettingsPanel from './settings-panel.svelte';
import { listen } from '@tauri-apps/api/event';
import { settingsStore, createDefaultSettings } from '../stores/settings.svelte';
import { getSettings, updateSettings, getSettingsLoadError, resetSettings, listInputDevices, checkAllPermissions, getLlmStatus, prepareLlmModel, getVocabularyStatus, getModelStatus, getAwsTranscribeOptions, getBedrockCleanupOptions } from '../utils/tauri';
import type { LlmStatus } from '../utils/tauri';
const status:LlmStatus = { available:true, unavailable_reason:null, downloaded:false, downloading:false, loaded:false, preparing:false, setup_error:null, model_name:'Test cleanup', model_path:null, model_url:'https://example.com/model', download_size_mb:227, last_cleanup_status:{kind:'idle'} };
let component:ReturnType<typeof mount>|undefined;
let target:HTMLDivElement;
beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(listen).mockImplementation(async (name, callback) => { listeners.set(name, callback as (event:unknown)=>void); return () => { listeners.delete(name); }; });
  vi.mocked(getModelStatus).mockResolvedValue({loaded:true,downloaded:true,path:null,name:'Parakeet',size_bytes:null});
  vi.mocked(getSettings).mockResolvedValue(createDefaultSettings());
  vi.mocked(getSettingsLoadError).mockResolvedValue(null);
  vi.mocked(listInputDevices).mockResolvedValue([{ name:'MacBook Pro Microphone', is_default:true }]);
  vi.mocked(checkAllPermissions).mockResolvedValue({ microphone:'authorized', accessibility_api:true, accessibility_functional:true, needs_restart:false });
  vi.mocked(updateSettings).mockImplementation(async (settings) => ({ settings, warnings:[] }));
  vi.mocked(getLlmStatus).mockResolvedValue({...status});
  vi.mocked(getVocabularyStatus).mockResolvedValue({supported:true, downloaded:false, loaded:false, preparing:false, download_size_mb:300, error:null});
  vi.mocked(getAwsTranscribeOptions).mockResolvedValue({ profiles:[{ name:'work', region:'us-west-2' }], profiles_error:null, regions:[{ code:'us-west-2', name:'US West (Oregon)' }], languages:['en-US'] });
  vi.mocked(getBedrockCleanupOptions).mockResolvedValue({ profiles:[{ name:'work', region:'us-west-2' }], profiles_error:null, models:[{ id:'haiku-5.5', name:'Claude Haiku 5.5' }],
    regions:[{ code:'us-west-2', name:'US West (Oregon)', routes:[{ model:'haiku-5.5', inference_profile:'us.anthropic.claude-haiku-5-5', processed_in:'AWS processes the text in the United States.' }] }] });
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
// Buttons inside a closed dialog are not visible to the user, so they are never the target.
const button = (label:string) => Array.from(target.querySelectorAll('button')).find((item) => item.textContent?.trim() === label && !item.closest('dialog:not([open])'))!;
const closeDialog = () => Array.from(target.querySelectorAll('dialog')).find((item) => item.textContent?.includes('Save your changes?'))!;
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
  expect(closeDialog().open).toBe(false);
});

it.each(['Discard', 'Save and close'])('protects edits on X and then permits %s to close', async (action) => {
  await render(); settingsStore.update('show_overlay', false); flushSync();
  const request = onClose.mock.calls[0][0];
  const event = { preventDefault: vi.fn() };
  request(event); flushSync();
  expect(event.preventDefault).toHaveBeenCalledOnce();
  expect(closeDialog().open).toBe(true);
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

it('blocks Save while a replacement row is incomplete and says why', async () => {
  await render(); button('Vocabulary').click(); flushSync();
  button('Add replacement').click(); flushSync(); type(input('Heard alias 1'), 'Quen');
  expect(button('Save').disabled).toBe(true);
  expect(target.querySelector('.footer-feedback')?.textContent).toContain('incomplete replacement');
  expect(target.querySelector('.row-error')?.textContent).toContain('Add the text to write instead');
  type(input('Replacement 1'), 'Qwen');
  expect(button('Save').disabled).toBe(false);
  button('Save').click();
  await vi.waitFor(() => expect(updateSettings).toHaveBeenCalledOnce());
  expect(vi.mocked(updateSettings).mock.calls[0][0].dictionary).toEqual([{ heard:'Quen', replacement:'Qwen' }]);
});

it('offers Back up and reset when the settings file is unreadable', async () => {
  vi.mocked(getSettings).mockRejectedValueOnce('Your saved settings could not be loaded.');
  vi.mocked(getSettingsLoadError).mockResolvedValueOnce('expected value at line 1 column 1');
  vi.mocked(resetSettings).mockResolvedValueOnce(createDefaultSettings());
  await render();
  expect(target.textContent).toContain('could not be loaded');
  button('Back up and reset').click(); flushSync();
  const dialog = Array.from(target.querySelectorAll('dialog')).find((item) => item.open)!;
  Array.from(dialog.querySelectorAll('button')).find((item) => item.textContent?.trim() === 'Back up and reset')!.click();
  await vi.waitFor(() => expect(target.querySelector('.footer-feedback')?.textContent).toContain('kept as a backup'));
  expect(settingsStore.loaded).toBe(true);
  expect(resetSettings).toHaveBeenCalledOnce();
});

it('shows the written defaults with the reason when reset fails after writing them', async () => {
  vi.mocked(getSettings).mockRejectedValueOnce('Your saved settings could not be loaded.');
  vi.mocked(getSettingsLoadError).mockResolvedValueOnce('expected value at line 1 column 1');
  vi.mocked(resetSettings).mockRejectedValueOnce('Settings were reset, but keyboard shortcuts could not be turned on. Choose other shortcuts and save.');
  await render();
  button('Back up and reset').click(); flushSync();
  Array.from(Array.from(target.querySelectorAll('dialog')).find((item) => item.open)!.querySelectorAll('button')).find((item) => item.textContent?.trim() === 'Back up and reset')!.click();
  await vi.waitFor(() => expect(target.querySelector('.footer-feedback')?.textContent).toContain('Settings were reset, but keyboard shortcuts'));
  expect(target.textContent).toContain('Keyboard shortcuts');
  expect(button('Back up and reset')).toBeUndefined();
  expect(getSettings).toHaveBeenCalledTimes(2);
});

it('keeps a saved microphone that is not connected and marks it unavailable', async () => {
  vi.mocked(getSettings).mockResolvedValue({ ...createDefaultSettings(), input_device:'USB Microphone' });
  await render(); button('Dictation').click();
  await vi.waitFor(() => expect(target.textContent).toContain('not connected')); flushSync();
  const select = target.querySelector<HTMLSelectElement>('select')!;
  expect(Array.from(select.options).map((option) => option.textContent)).toEqual(['System default (MacBook Pro Microphone)', 'MacBook Pro Microphone', 'USB Microphone (not connected)']);
  expect(select.value).toBe('USB Microphone');
  expect(settingsStore.dirty).toBe(false);
  select.value = ''; select.dispatchEvent(new Event('change', { bubbles:true })); flushSync();
  expect(settingsStore.current.input_device).toBeNull();
});

it('blocks Save until AWS Transcribe has a profile, and says so when the window closes', async () => {
  await render(); button('Dictation').click(); flushSync();
  expect(target.querySelector('h3')?.textContent).toBe('Speech recognition');
  target.querySelector<HTMLInputElement>('input[name="speech-engine"][value="aws_transcribe"]')!.click(); flushSync();
  expect(button('Save').disabled).toBe(true);
  expect(target.querySelector('.footer-feedback')?.textContent).toContain('Choose an AWS profile for AWS Transcribe.');
  onClose.mock.calls[0][0]({ preventDefault:vi.fn() }); flushSync();
  expect(closeDialog().textContent).toContain('AWS Transcribe needs an AWS profile. Choose one in Dictation to save, or discard this draft.');
  button('Keep editing').click(); flushSync();
  const profile = target.querySelector<HTMLSelectElement>('select')!;
  await vi.waitFor(() => expect(Array.from(profile.options).map((option) => option.value)).toEqual(['', 'work']));
  profile.value = 'work'; profile.dispatchEvent(new Event('change', { bubbles:true })); flushSync();
  expect(button('Save').disabled).toBe(false);
  button('Save').click();
  await vi.waitFor(() => expect(updateSettings).toHaveBeenCalledOnce());
  expect(vi.mocked(updateSettings).mock.calls[0][0]).toMatchObject({ speech_engine:'aws_transcribe', aws_transcribe:{ profile:'work', region:null, language:'en-US', vocabulary_name:null } });
});

it('blocks Save while cleanup is on with AWS Bedrock and no profile, and says so when the window closes', async () => {
  await render(); button('Dictation').click(); flushSync();
  target.querySelector<HTMLInputElement>('input[name="cleanup-engine"][value="aws_bedrock"]')!.click(); flushSync();
  // With cleanup off, the draft needs no profile.
  expect(button('Save').disabled).toBe(false);
  cleanupToggle().click(); flushSync();
  expect(prepareLlmModel).not.toHaveBeenCalled();
  expect(button('Save').disabled).toBe(true);
  expect(target.querySelector('.footer-feedback')?.textContent).toContain('Choose an AWS profile for AWS Bedrock.');
  onClose.mock.calls[0][0]({ preventDefault:vi.fn() }); flushSync();
  expect(closeDialog().textContent).toContain('AWS Bedrock needs an AWS profile. Choose one in Dictation to save, or discard this draft.');
  button('Keep editing').click(); flushSync();
  const profile = Array.from(target.querySelectorAll('label.setting-field')).find((item) => item.querySelector('.field-label')?.textContent === 'AWS profile')!.querySelector('select')!;
  await vi.waitFor(() => expect(Array.from(profile.options).map((option) => option.value)).toEqual(['', 'work']));
  profile.value = 'work'; profile.dispatchEvent(new Event('change', { bubbles:true })); flushSync();
  expect(target.textContent).toContain('Inference profile us.anthropic.claude-haiku-5-5. AWS processes the text in the United States.');
  expect(button('Save').disabled).toBe(false);
  button('Save').click();
  await vi.waitFor(() => expect(updateSettings).toHaveBeenCalledOnce());
  expect(vi.mocked(updateSettings).mock.calls[0][0]).toMatchObject({ llm_cleanup_enabled:true, cleanup_engine:'aws_bedrock', aws_bedrock:{ profile:'work', region:null, model:'haiku-5.5' } });
});

it('lets a user of AWS Bedrock remove the local cleanup model', async () => {
  vi.mocked(getLlmStatus).mockResolvedValue({ ...status, downloaded:true, engine:'aws_bedrock' });
  vi.mocked(getSettings).mockResolvedValue({ ...createDefaultSettings(), llm_cleanup_enabled:true, cleanup_engine:'aws_bedrock', aws_bedrock:{ profile:'work', region:null, model:'haiku-5.5' } });
  await render(); button('Advanced').click();
  await vi.waitFor(() => expect(button('Remove cleanup model')).toBeDefined());
  expect(button('Remove cleanup model').disabled).toBe(false);
  // The window does not say that the text stays on the Mac.
  expect(target.textContent).toContain('Speech recognition uses Parakeet Ultra. AI cleanup uses AWS Bedrock. SottoASR sends dictation text to AWS. History and kept recordings stay on your Mac.');
  expect(target.textContent).not.toContain('transcripts stay on your Mac');
  expect(target.textContent).not.toContain('Turn off cleanup and Save before removing its model.');
  // The local engine in the draft uses the model.
  settingsStore.update('cleanup_engine', 'local'); flushSync();
  expect(button('Remove cleanup model').disabled).toBe(true);
  expect(target.textContent).toContain('Turn off cleanup and Save before removing its model.');
});

it('warns before a retention choice deletes older transcripts and recordings', async () => {
  await render(); button('Advanced').click(); flushSync();
  expect(target.textContent).toContain('including recordings waiting for recovery');
  const [history, audio] = Array.from(target.querySelectorAll<HTMLSelectElement>('select'));
  expect(Array.from(history.options).map((option) => option.textContent)).toEqual(['Keep until I delete', '1 day', '7 days', '30 days', '90 days', '365 days']);
  history.value = '7'; history.dispatchEvent(new Event('change', { bubbles:true })); flushSync();
  audio.value = '30'; audio.dispatchEvent(new Event('change', { bubbles:true })); flushSync();
  expect(settingsStore.current.history_retention_days).toBe(7);
  expect(settingsStore.current.audio_retention_days).toBe(30);
  expect(target.textContent).toContain('Saving deletes transcripts older than 7 days and the audio kept with them.');
  expect(target.textContent).toContain('Saving deletes recordings older than 30 days, including recordings waiting for recovery.');
});

it('shows the saved retention again after two picks and Cancel', async () => {
  await render(); button('Advanced').click(); flushSync();
  const history = target.querySelector<HTMLSelectElement>('select')!;
  history.value = '7'; history.dispatchEvent(new Event('change', { bubbles:true })); flushSync();
  history.value = '30'; history.dispatchEvent(new Event('change', { bubbles:true })); flushSync();
  expect(settingsStore.current.history_retention_days).toBe(30);
  button('Cancel').click(); flushSync();
  expect(settingsStore.current.history_retention_days).toBe(0);
  expect(history.value).toBe('0');
  expect(history.selectedOptions[0].textContent).toBe('Keep until I delete');
});
