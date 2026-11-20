import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { flushSync, mount, unmount } from 'svelte';
vi.mock('../utils/tauri', () => ({
  getBedrockCleanupOptions:vi.fn(), testBedrockCleanup:vi.fn(), openUrl:vi.fn(async () => undefined), listInputDevices:vi.fn(),
  getLlmStatus:vi.fn(), prepareLlmModel:vi.fn(), repairLlmRuntime:vi.fn(), getAwsTranscribeOptions:vi.fn(), testAwsTranscribe:vi.fn(),
}));
import { getBedrockCleanupOptions, listInputDevices, openUrl, prepareLlmModel, testBedrockCleanup } from '../utils/tauri';
import type { BedrockCleanupOptions, BedrockCleanupTest, LlmStatus, Settings } from '../utils/tauri';
import { createDefaultSettings, settingsStore } from '../stores/settings.svelte';
import { CleanupSetup } from '../stores/cleanup-setup.svelte';
import SettingsDictation from './settings-dictation.svelte';
const US = 'AWS processes the text in the United States.';
const EU = 'AWS processes the text in the European Union.';
const GLOBAL = 'AWS can process the text in any commercial AWS Region.';
const bedrockOptions:BedrockCleanupOptions = {
  profiles:[{ name:'default', region:'us-west-2' }, { name:'work', region:null }, { name:'hongkong', region:'ap-east-1' }], profiles_error:null,
  regions:[
    { code:'us-west-2', name:'US West (Oregon)', routes:[
      { model:'haiku-5.5', inference_profile:'us.anthropic.claude-haiku-5-5', processed_in:US },
      { model:'gpt-6.2-sol', inference_profile:'us.openai.gpt-6.2-sol', processed_in:US },
    ] },
    { code:'eu-central-1', name:'Europe (Frankfurt)', routes:[
      { model:'haiku-5.5', inference_profile:'eu.anthropic.claude-haiku-5-5', processed_in:EU },
      { model:'gpt-6.2-sol', inference_profile:'global.openai.gpt-6.2-sol', processed_in:GLOBAL },
    ] },
  ],
  models:[{ id:'haiku-5.5', name:'Claude Haiku 5.5' }, { id:'sonnet-5.5', name:'Claude Sonnet 5.5' }, { id:'opus-5.5', name:'Claude Opus 5.5' }, { id:'gpt-6.2-sol', name:'GPT-6.2 Sol' }],
};
const localStatus:LlmStatus = { available:true, unavailable_reason:null, downloaded:false, downloading:false, loaded:false, preparing:false, setup_error:null, model_name:'MiniCPM5 2B', model_url:'https://example.com/model', model_path:null, download_size_mb:1427, last_cleanup_status:{ kind:'idle' }, engine:'local', notice:null, cloud_error:null };
const loadedStatus:LlmStatus = { ...localStatus, downloaded:true, loaded:true };
let component:ReturnType<typeof mount>|undefined;
let target:HTMLDivElement;
let cleanup:CleanupSetup;
beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(getBedrockCleanupOptions).mockResolvedValue(bedrockOptions);
  vi.mocked(listInputDevices).mockResolvedValue([]);
  settingsStore.current = createDefaultSettings(); settingsStore.saved = createDefaultSettings(); settingsStore.loaded = true;
  cleanup = new CleanupSetup();
  cleanup.status = { ...localStatus };
});
afterEach(async () => { if (component) await unmount(component); component = undefined; cleanup.dispose(); target?.remove(); });
function render(change?:Partial<Settings>) {
  if (change) settingsStore.current = { ...createDefaultSettings(), ...change };
  target = document.createElement('div'); document.body.append(target); component = mount(SettingsDictation, { target, props:{ cleanup } }); flushSync();
}
const card = () => Array.from(target.querySelectorAll('section')).find((section) => section.querySelector('h3')?.textContent === 'Transcript cleanup')!;
const engine = (value:string) => card().querySelector<HTMLInputElement>(`input[name="cleanup-engine"][value="${value}"]`)!;
const toggle = () => Array.from(card().querySelectorAll('label')).find((label) => label.textContent?.includes('AI transcript cleanup'))!.querySelector<HTMLInputElement>('input')!;
const field = (label:string) => Array.from(card().querySelectorAll('label.setting-field')).find((item) => item.querySelector('.field-label')?.textContent === label)?.querySelector('select') ?? null;
const select = (label:string) => field(label)!;
const labels = (label:string) => Array.from(select(label).options).map((option) => option.textContent);
const button = (label:string) => Array.from(card().querySelectorAll('button')).find((item) => item.textContent?.trim() === label);
const routeText = () => Array.from(card().querySelectorAll('p')).find((item) => item.textContent?.startsWith('Inference profile'))?.textContent ?? null;
function choose(label:string, value:string) { const element = select(label); element.value = value; element.dispatchEvent(new Event('change', { bubbles:true })); flushSync(); }
// A select ignores a value that is not yet one of its options, so a test waits for the AWS lists.
const listed = () => vi.waitFor(() => expect(labels('AWS profile')).toContain('work'));

it('shows the two engines with the local one selected, also while cleanup is off, and reads no AWS files', () => {
  render();
  expect(toggle().checked).toBe(false);
  expect(card().textContent).toContain('Accepted cleanup is applied automatically. The original remains in History. Off by default.');
  expect(card().textContent).toContain('Cleanup engine');
  expect(engine('local').checked).toBe(true);
  expect(engine('aws_bedrock').checked).toBe(false);
  expect(card().textContent).toContain('MiniCPM5 2B. The text stays on this Mac.');
  expect(card().textContent).toContain('A cloud service of Amazon Web Services. No cleanup model is in memory. Needs an AWS account and an internet connection.');
  expect(card().querySelector('select')).toBeNull();
  expect(button('Test connection')).toBeUndefined();
  window.dispatchEvent(new Event('focus'));
  expect(getBedrockCleanupOptions).not.toHaveBeenCalled();
});

it('shows the AWS options after AWS Bedrock is selected, with the route of the effective Region and model', async () => {
  render(); engine('aws_bedrock').click(); flushSync();
  expect(settingsStore.current.cleanup_engine).toBe('aws_bedrock');
  await listed();
  expect(getBedrockCleanupOptions).toHaveBeenCalledOnce();
  expect(labels('AWS profile')).toEqual(['Choose a profile', 'default', 'work', 'hongkong']);
  expect(labels('Region')).toEqual(['Choose a Region', 'US West (Oregon) (us-west-2)', 'Europe (Frankfurt) (eu-central-1)']);
  expect(labels('Model')).toEqual(['Claude Haiku 5.5', 'Claude Sonnet 5.5', 'Claude Opus 5.5', 'GPT-6.2 Sol']);
  expect(select('Model').value).toBe('haiku-5.5');
  expect(routeText()).toBeNull();
  expect(card().textContent).toContain('AWS bills your account for the tokens. Amazon Bedrock does not give the text to the model provider. SottoASR examines each change that the model proposes, and it can only remove words.');
  expect(card().textContent).not.toContain('abuse classifier');

  choose('AWS profile', 'default');
  expect(settingsStore.current.aws_bedrock.profile).toBe('default');
  expect(labels('Region')[0]).toBe('From the profile (us-west-2)');
  expect(routeText()).toBe(`Inference profile us.anthropic.claude-haiku-5-5. ${US}`);
  choose('Region', 'eu-central-1');
  expect(routeText()).toBe(`Inference profile eu.anthropic.claude-haiku-5-5. ${EU}`);
  choose('Model', 'gpt-6.2-sol');
  expect(routeText()).toBe(`Inference profile global.openai.gpt-6.2-sol. ${GLOBAL}`);
  expect(card().textContent).toContain('it can only remove words. For this model, AWS can keep text that its abuse classifier flags for up to 30 days.');

  // The Region of this profile has no cleanup model, so the window offers no Region of the profile.
  choose('AWS profile', 'hongkong'); choose('Region', '');
  expect(labels('Region')[0]).toBe('Choose a Region');
  expect(routeText()).toBeNull();
  expect(settingsStore.current.aws_bedrock).toEqual({ profile:'hongkong', region:null, model:'gpt-6.2-sol' });

  // Cleanup is off, so the local engine needs no setup. The AWS values stay for later.
  engine('local').click(); flushSync();
  expect(settingsStore.current.cleanup_engine).toBe('local');
  expect(prepareLlmModel).not.toHaveBeenCalled();
  expect(field('AWS profile')).toBeNull();
  expect(settingsStore.current.aws_bedrock).toEqual({ profile:'hongkong', region:null, model:'gpt-6.2-sol' });
});

it('turns cleanup on with the local engine only after the local setup succeeds', async () => {
  let ready!:(status:LlmStatus)=>void;
  vi.mocked(prepareLlmModel).mockReturnValueOnce(new Promise((resolve) => { ready = resolve; }));
  render();
  expect(card().textContent).toContain('Turning this on handles setup automatically, including a one-time 1427 MB download.');
  toggle().click(); flushSync();
  expect(prepareLlmModel).toHaveBeenCalledOnce();
  expect(card().querySelector('[role="status"]')?.textContent).toContain('Preparing local cleanup…');
  expect(settingsStore.current.llm_cleanup_enabled).toBe(false);
  ready(loadedStatus);
  await vi.waitFor(() => expect(settingsStore.current.llm_cleanup_enabled).toBe(true));
  expect(settingsStore.current.cleanup_engine).toBe('local');
});

it('turns cleanup on with AWS Bedrock at once, without the local setup', () => {
  render({ cleanup_engine:'aws_bedrock' });
  // The local model is not downloaded, but its setup does not apply to AWS Bedrock.
  expect(card().textContent).not.toContain('Turning this on handles setup automatically');
  toggle().click(); flushSync();
  expect(settingsStore.current.llm_cleanup_enabled).toBe(true);
  expect(toggle().checked).toBe(true);
  expect(prepareLlmModel).not.toHaveBeenCalled();
  expect(card().querySelector('[role="status"]')).toBeNull();
});

it('disables the toggle on a Mac without local cleanup only for the local engine', () => {
  cleanup.status = { ...localStatus, available:false, unavailable_reason:'Local cleanup needs a Mac with Apple silicon.' };
  render();
  expect(toggle().disabled).toBe(true);
  expect(card().textContent).toContain('Local cleanup needs a Mac with Apple silicon.');
  engine('aws_bedrock').click(); flushSync();
  expect(toggle().disabled).toBe(false);
  expect(card().textContent).not.toContain('Local cleanup needs a Mac with Apple silicon.');
});

it('cancels a local setup in flight when AWS Bedrock is selected', async () => {
  let ready!:(status:LlmStatus)=>void;
  const preparation = new Promise<LlmStatus>((resolve) => { ready = resolve; });
  vi.mocked(prepareLlmModel).mockReturnValueOnce(preparation);
  render(); toggle().click(); flushSync();
  expect(card().textContent).toContain('Preparing local cleanup…');
  engine('aws_bedrock').click(); flushSync();
  expect(settingsStore.current.cleanup_engine).toBe('aws_bedrock');
  expect(card().textContent).not.toContain('Preparing local cleanup…');
  // The store awaits the same promise first, so its cancelled continuation has run.
  ready(loadedStatus); await preparation; flushSync();
  expect(settingsStore.current.llm_cleanup_enabled).toBe(false);
  expect(toggle().checked).toBe(false);
});

it('prepares the local model before the draft changes from AWS Bedrock to the local engine', async () => {
  let ready!:(status:LlmStatus)=>void;
  vi.mocked(prepareLlmModel).mockReturnValueOnce(new Promise((resolve) => { ready = resolve; }));
  cleanup.status = { ...localStatus, downloaded:true };
  render({ llm_cleanup_enabled:true, cleanup_engine:'aws_bedrock', aws_bedrock:{ profile:'default', region:null, model:'haiku-5.5' } });
  await listed();
  engine('local').click(); flushSync();
  expect(prepareLlmModel).toHaveBeenCalledOnce();
  expect(settingsStore.current.cleanup_engine).toBe('aws_bedrock');
  // The window shows the choice and its setup, not the AWS options.
  expect(engine('local').checked).toBe(true);
  expect(field('AWS profile')).toBeNull();
  expect(card().textContent).toContain('Preparing local cleanup…');
  ready(loadedStatus);
  await vi.waitFor(() => expect(settingsStore.current.cleanup_engine).toBe('local'));
  flushSync();
  expect(engine('local').checked).toBe(true);
  expect(settingsStore.current.llm_cleanup_enabled).toBe(true);
  expect(card().textContent).not.toContain('Preparing local cleanup…');
});

it('keeps AWS Bedrock and says why when the local setup for the change fails', async () => {
  vi.mocked(prepareLlmModel).mockRejectedValueOnce(new Error('Network unavailable'));
  render({ llm_cleanup_enabled:true, cleanup_engine:'aws_bedrock', aws_bedrock:{ profile:'default', region:null, model:'haiku-5.5' } });
  await listed();
  engine('local').click();
  await vi.waitFor(() => expect(card().querySelector('[role="alert"]')?.textContent).toBe('Network unavailable'));
  expect(settingsStore.current.cleanup_engine).toBe('aws_bedrock');
  expect(engine('aws_bedrock').checked).toBe(true);
  expect(engine('local').checked).toBe(false);
  expect(select('AWS profile').value).toBe('default');
});

it('changes the draft to the local engine at once when the local model is loaded', () => {
  cleanup.status = { ...loadedStatus };
  render({ llm_cleanup_enabled:true, cleanup_engine:'aws_bedrock' });
  engine('local').click(); flushSync();
  expect(settingsStore.current.cleanup_engine).toBe('local');
  expect(prepareLlmModel).not.toHaveBeenCalled();
});

it('tests the options in the window and shows the answer until an option changes', async () => {
  let answer!:(result:BedrockCleanupTest)=>void;
  vi.mocked(testBedrockCleanup).mockReturnValueOnce(new Promise((resolve) => { answer = resolve; }));
  // An empty Region in the window means the Region of the profile.
  render({ cleanup_engine:'aws_bedrock', aws_bedrock:{ profile:'default', region:'', model:'haiku-5.5' } });
  await listed();
  expect(settingsStore.saved?.aws_bedrock.profile).toBeNull();
  button('Test connection')!.click(); flushSync();
  expect(testBedrockCleanup).toHaveBeenCalledOnce();
  expect(testBedrockCleanup).toHaveBeenCalledWith({ profile:'default', region:null, model:'haiku-5.5' });
  expect(button('Testing…')!.disabled).toBe(true);
  answer({ region:'us-west-2', model:'Claude Haiku 5.5', inference_profile:'us.anthropic.claude-haiku-5-5', seconds:0.66 });
  await vi.waitFor(() => expect(card().querySelector('[role="status"]')?.textContent).toBe('Amazon Bedrock answered in 0.7 s (Claude Haiku 5.5, us-west-2).'));
  expect(button('Test connection')!.disabled).toBe(false);

  choose('Model', 'gpt-6.2-sol');
  expect(card().querySelector('[role="status"]')).toBeNull();
});

it('shows why a connection test failed, clears it when the profile changes, and needs a listed Region', async () => {
  const denied = 'AWS profile default cannot use Claude Haiku 5.5 in Amazon Bedrock. It needs the permission bedrock:InvokeModel and access to the model.';
  vi.mocked(testBedrockCleanup).mockRejectedValueOnce(denied);
  render({ cleanup_engine:'aws_bedrock', aws_bedrock:{ profile:'default', region:null, model:'haiku-5.5' } });
  await listed();
  button('Test connection')!.click();
  await vi.waitFor(() => expect(card().querySelector('[role="alert"]')?.textContent).toBe(denied));
  choose('AWS profile', 'hongkong');
  expect(card().querySelector('[role="alert"]')).toBeNull();
  // The Region of this profile has no cleanup model.
  expect(button('Test connection')!.disabled).toBe(true);
  choose('Region', 'eu-central-1');
  expect(button('Test connection')!.disabled).toBe(false);
});

it('keeps a saved profile that the AWS files no longer have, and says when they have none', async () => {
  vi.mocked(getBedrockCleanupOptions).mockResolvedValue({ ...bedrockOptions, profiles:[] });
  render({ cleanup_engine:'aws_bedrock', aws_bedrock:{ profile:'old', region:null, model:'haiku-5.5' } });
  await vi.waitFor(() => expect(card().textContent).toContain('No AWS profile was found in ~/.aws/config or ~/.aws/credentials.'));
  expect(labels('AWS profile')).toEqual(['old (not found)']);
  expect(select('AWS profile').value).toBe('old');
  // Neither the settings nor a known profile give a Region.
  expect(button('Test connection')!.disabled).toBe(true);
});

it('shows why the AWS files could not be read, and reads them again on focus and on request', async () => {
  vi.mocked(getBedrockCleanupOptions).mockResolvedValueOnce({ ...bedrockOptions, profiles:[], profiles_error:'Could not read ~/.aws/config: line 3 is not valid.' });
  render({ cleanup_engine:'aws_bedrock' });
  await vi.waitFor(() => expect(card().querySelector('[role="alert"]')?.textContent).toBe('Could not read ~/.aws/config: line 3 is not valid.'));
  expect(card().textContent).not.toContain('No AWS profile was found');

  vi.mocked(getBedrockCleanupOptions).mockRejectedValueOnce('The AWS options are unavailable.');
  window.dispatchEvent(new Event('focus'));
  await vi.waitFor(() => expect(card().textContent).toContain('The AWS options are unavailable.'));
  button('Try again')!.click();
  await listed();
  expect(card().querySelector('[role="alert"]')).toBeNull();
  expect(getBedrockCleanupOptions).toHaveBeenCalledTimes(3);
});

it('opens the AWS pages in the browser', async () => {
  render({ cleanup_engine:'aws_bedrock' });
  await listed();
  Array.from(card().querySelectorAll('a')).find((link) => link.textContent === 'How Amazon Bedrock protects your data')!.click();
  Array.from(card().querySelectorAll('a')).find((link) => link.textContent === 'Pricing')!.click();
  expect(vi.mocked(openUrl).mock.calls).toEqual([['https://docs.aws.amazon.com/bedrock/latest/userguide/data-protection.html'], ['https://aws.amazon.com/bedrock/pricing/']]);
});
