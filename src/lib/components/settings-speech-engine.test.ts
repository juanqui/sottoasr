import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { flushSync, mount, unmount } from 'svelte';
vi.mock('../utils/tauri', () => ({ getAwsTranscribeOptions:vi.fn(), testAwsTranscribe:vi.fn(), openUrl:vi.fn(async () => undefined) }));
import { getAwsTranscribeOptions, openUrl, testAwsTranscribe } from '../utils/tauri';
import type { AwsTranscribeOptions, AwsTranscribeSettings, AwsTranscribeTest } from '../utils/tauri';
import { createDefaultSettings, settingsStore } from '../stores/settings.svelte';
import SettingsSpeechEngine from './settings-speech-engine.svelte';
const awsOptions:AwsTranscribeOptions = {
  profiles:[{ name:'default', region:'us-west-2' }, { name:'work', region:null }], profiles_error:null,
  regions:[{ code:'us-east-1', name:'US East (N. Virginia)' }, { code:'us-west-2', name:'US West (Oregon)' }],
  languages:['fr-FR', 'en-US', 'de-DE'],
};
let component:ReturnType<typeof mount>|undefined;
let target:HTMLDivElement;
beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(getAwsTranscribeOptions).mockResolvedValue(awsOptions);
  settingsStore.current = createDefaultSettings(); settingsStore.saved = createDefaultSettings(); settingsStore.loaded = true;
});
afterEach(async () => { if (component) await unmount(component); component = undefined; target?.remove(); });
function render(aws?:AwsTranscribeSettings) {
  if (aws) settingsStore.current = { ...createDefaultSettings(), speech_engine:'aws_transcribe', aws_transcribe:aws };
  target = document.createElement('div'); document.body.append(target); component = mount(SettingsSpeechEngine, {target}); flushSync();
}
const engine = (value:string) => target.querySelector<HTMLInputElement>(`input[name="speech-engine"][value="${value}"]`)!;
const field = <T extends HTMLElement>(label:string) => Array.from(target.querySelectorAll('label.setting-field')).find((item) => item.querySelector('.field-label')?.textContent === label)?.querySelector<T>('select, input') ?? null;
const select = (label:string) => field<HTMLSelectElement>(label)!;
const labels = (label:string) => Array.from(select(label).options).map((option) => option.textContent);
const button = (label:string) => Array.from(target.querySelectorAll('button')).find((item) => item.textContent?.trim() === label)!;
function choose(label:string, value:string) { const element = select(label); element.value = value; element.dispatchEvent(new Event('change', { bubbles:true })); flushSync(); }
function type(label:string, value:string) { const element = field<HTMLInputElement>(label)!; element.value = value; element.dispatchEvent(new Event('input', { bubbles:true })); flushSync(); }
const profileListed = () => vi.waitFor(() => expect(labels('AWS profile')).toContain('work'));

it('shows only the two engines in local mode and reads no AWS files', () => {
  render();
  expect(engine('local').checked).toBe(true);
  expect(engine('aws_transcribe').checked).toBe(false);
  expect(target.textContent).toContain('Parakeet Ultra. The audio stays on this Mac.');
  expect(target.querySelector('select')).toBeNull();
  expect(target.querySelector('input:not([type="radio"])')).toBeNull();
  expect(button('Test connection')).toBeUndefined();
  expect(getAwsTranscribeOptions).not.toHaveBeenCalled();
});

it('shows the AWS options after AWS Transcribe is selected and writes each one to the draft', async () => {
  render(); engine('aws_transcribe').click(); flushSync();
  expect(settingsStore.current.speech_engine).toBe('aws_transcribe');
  await profileListed();
  expect(getAwsTranscribeOptions).toHaveBeenCalledOnce();
  expect(labels('AWS profile')).toEqual(['Choose a profile', 'default', 'work']);
  expect(labels('Region')).toEqual(['Choose a Region', 'US East (N. Virginia) (us-east-1)', 'US West (Oregon) (us-west-2)']);
  expect(target.textContent).toContain('A cancelled recording is not sent.');

  choose('AWS profile', 'default');
  expect(settingsStore.current.aws_transcribe.profile).toBe('default');
  expect(labels('AWS profile')).toEqual(['default', 'work']);
  expect(labels('Region')[0]).toBe('From the profile (us-west-2)');
  expect(target.textContent).not.toContain('This profile has no Region');

  choose('AWS profile', 'work');
  expect(labels('Region')[0]).toBe('Choose a Region');
  expect(target.textContent).toContain('This profile has no Region. Choose one.');
  choose('Region', 'us-east-1');
  expect(settingsStore.current.aws_transcribe.region).toBe('us-east-1');
  expect(target.textContent).not.toContain('This profile has no Region');

  // Sorted by the language name, not by the code.
  expect(Array.from(select('Language').options).map((option) => option.value)).toEqual(['en-US', 'fr-FR', 'de-DE']);
  expect(labels('Language')[0]).toBe('English (United States) (en-US)');
  expect(select('Language').value).toBe('en-US');
  choose('Language', 'de-DE');
  type('Custom vocabulary', 'medical-terms');
  expect(settingsStore.current.aws_transcribe).toEqual({ profile:'work', region:'us-east-1', language:'de-DE', vocabulary_name:'medical-terms' });

  // Going back to the local engine keeps the AWS values for later.
  engine('local').click(); flushSync();
  expect(settingsStore.current.speech_engine).toBe('local');
  expect(target.querySelector('select')).toBeNull();
  expect(settingsStore.current.aws_transcribe).toEqual({ profile:'work', region:'us-east-1', language:'de-DE', vocabulary_name:'medical-terms' });
});

it('keeps a saved profile that the AWS files no longer have, and says when they have none', async () => {
  vi.mocked(getAwsTranscribeOptions).mockResolvedValue({ ...awsOptions, profiles:[] });
  render({ profile:'old', region:null, language:'en-US', vocabulary_name:null });
  await vi.waitFor(() => expect(target.textContent).toContain('No AWS profile was found in ~/.aws/config or ~/.aws/credentials.'));
  expect(labels('AWS profile')).toEqual(['old (not found)']);
  expect(select('AWS profile').value).toBe('old');
  // Neither the settings nor a known profile give a Region.
  expect(button('Test connection').disabled).toBe(true);
});

it('shows why the AWS files could not be read, and loads them again on request', async () => {
  vi.mocked(getAwsTranscribeOptions).mockResolvedValueOnce({ ...awsOptions, profiles:[], profiles_error:'Could not read ~/.aws/config: line 3 is not valid.' });
  render({ profile:null, region:null, language:'en-US', vocabulary_name:null });
  await vi.waitFor(() => expect(target.querySelector('[role="alert"]')?.textContent).toBe('Could not read ~/.aws/config: line 3 is not valid.'));
  expect(target.textContent).not.toContain('No AWS profile was found');

  vi.mocked(getAwsTranscribeOptions).mockRejectedValueOnce('The AWS options are unavailable.');
  window.dispatchEvent(new Event('focus'));
  await vi.waitFor(() => expect(target.textContent).toContain('The AWS options are unavailable.'));
  button('Try again').click();
  await profileListed();
  expect(target.querySelector('[role="alert"]')).toBeNull();
  expect(getAwsTranscribeOptions).toHaveBeenCalledTimes(3);
});

it('tests the options in the window and shows the answer until an option changes', async () => {
  let answer!:(result:AwsTranscribeTest)=>void;
  vi.mocked(testAwsTranscribe).mockReturnValueOnce(new Promise<AwsTranscribeTest>((resolve) => { answer = resolve; }));
  render({ profile:'work', region:null, language:'en-US', vocabulary_name:null });
  // The saved profile is listed at once; the Regions come with the options.
  await vi.waitFor(() => expect(labels('Region')).toContain('US East (N. Virginia) (us-east-1)'));
  expect(button('Test connection').disabled).toBe(true);
  choose('Region', 'us-east-1'); type('Custom vocabulary', ' medical-terms ');
  expect(settingsStore.saved?.aws_transcribe.region).toBeNull();

  button('Test connection').click(); flushSync();
  expect(testAwsTranscribe).toHaveBeenCalledOnce();
  expect(testAwsTranscribe).toHaveBeenCalledWith({ profile:'work', region:'us-east-1', language:'en-US', vocabulary_name:'medical-terms' });
  expect(button('Testing…').disabled).toBe(true);
  answer({ region:'us-east-1', seconds:0.64 });
  await vi.waitFor(() => expect(target.querySelector('[role="status"]')?.textContent).toBe('Amazon Transcribe answered in 0.6 s (us-east-1).'));
  expect(button('Test connection').disabled).toBe(false);

  choose('Language', 'de-DE');
  expect(target.querySelector('[role="status"]')).toBeNull();
});

it('shows why a connection test failed, and clears it when the profile changes', async () => {
  vi.mocked(testAwsTranscribe).mockRejectedValueOnce('The credentials of AWS profile default are not valid or are expired. Sign in again.');
  render({ profile:'default', region:null, language:'en-US', vocabulary_name:null });
  await profileListed();
  button('Test connection').click();
  await vi.waitFor(() => expect(target.querySelector('[role="alert"]')?.textContent).toBe('The credentials of AWS profile default are not valid or are expired. Sign in again.'));
  expect(testAwsTranscribe).toHaveBeenCalledWith({ profile:'default', region:null, language:'en-US', vocabulary_name:null });
  choose('AWS profile', 'work');
  expect(target.querySelector('[role="alert"]')).toBeNull();
});

it('opens the AWS pages in the browser', async () => {
  render({ profile:'default', region:null, language:'en-US', vocabulary_name:null });
  await profileListed();
  Array.from(target.querySelectorAll('a')).find((link) => link.textContent === 'How AWS uses your audio')!.click();
  Array.from(target.querySelectorAll('a')).find((link) => link.textContent === 'Pricing')!.click();
  expect(vi.mocked(openUrl).mock.calls).toEqual([['https://docs.aws.amazon.com/transcribe/latest/dg/opt-out.html'], ['https://aws.amazon.com/transcribe/pricing/']]);
});
