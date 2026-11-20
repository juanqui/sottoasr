<script lang="ts">
  import { onMount } from 'svelte';
  import { normalizedAwsTranscribe, settingsStore } from '../stores/settings.svelte';
  import { getAwsTranscribeOptions, openUrl, testAwsTranscribe } from '../utils/tauri';
  import type { AwsTranscribeOptions, AwsTranscribeSettings } from '../utils/tauri';

  const DATA_USE_URL = 'https://docs.aws.amazon.com/transcribe/latest/dg/opt-out.html';
  const PRICING_URL = 'https://aws.amazon.com/transcribe/pricing/';
  let options = $state<AwsTranscribeOptions | null>(null);
  let optionsError = $state('');
  let optionsRequest = 0;
  let testing = $state(false);
  let testResult = $state<{ ok: boolean; text: string } | null>(null);
  let testRequest = 0;
  let linkError = $state('');
  let cloud = $derived(settingsStore.current.speech_engine === 'aws_transcribe');
  let aws = $derived(settingsStore.current.aws_transcribe);
  let profile = $derived(options?.profiles.find((item) => item.name === aws.profile));
  let profileRegion = $derived(profile?.region ?? null);
  let languages = $derived.by(() => {
    const label = (code: string) => {
      try {
        const name = new Intl.DisplayNames(['en'], { type: 'language', languageDisplay: 'standard' }).of(code);
        return name ? `${name} (${code})` : code;
      } catch { return code; }
    };
    return (options?.languages ?? []).map((code) => ({ code, label: label(code) })).sort((a, b) => a.label.localeCompare(b.label, 'en'));
  });

  async function refreshOptions() {
    const request = ++optionsRequest;
    try {
      const listed = await getAwsTranscribeOptions();
      if (request === optionsRequest) { options = listed; optionsError = ''; }
    } catch (error) {
      if (request === optionsRequest) optionsError = String(error);
    }
  }

  /** Write one AWS option to the draft. A result of the connection test no longer applies. */
  function updateAws(change: Partial<AwsTranscribeSettings>) {
    settingsStore.update('aws_transcribe', { ...settingsStore.current.aws_transcribe, ...change });
    ++testRequest;
    testResult = null;
  }

  async function testConnection() {
    const request = ++testRequest;
    testing = true;
    testResult = null;
    try {
      const result = await testAwsTranscribe(normalizedAwsTranscribe(settingsStore.current.aws_transcribe));
      if (request === testRequest) testResult = { ok: true, text: `Amazon Transcribe answered in ${result.seconds.toFixed(1)} s (${result.region}).` };
    } catch (error) {
      if (request === testRequest) testResult = { ok: false, text: String(error) };
    } finally {
      testing = false;
    }
  }

  function openLink(event: MouseEvent, url: string) {
    event.preventDefault();
    linkError = '';
    void openUrl(url).catch((error) => { linkError = String(error); });
  }

  // The AWS files are read only while AWS Transcribe is selected.
  $effect(() => { if (cloud && !options && !optionsError) void refreshOptions(); });

  onMount(() => {
    const refresh = () => { if (settingsStore.current.speech_engine === 'aws_transcribe') void refreshOptions(); };
    window.addEventListener('focus', refresh);
    return () => { ++optionsRequest; ++testRequest; window.removeEventListener('focus', refresh); };
  });
</script>

<section class="setting-card">
  <h3>Speech recognition</h3>
  <div class="mode-options" role="radiogroup" aria-label="Speech recognition">
    <label class="mode-option"><input type="radio" name="speech-engine" value="local"
      checked={!cloud}
      onchange={() => settingsStore.update('speech_engine', 'local')} />
      <span><strong>On this Mac</strong><span class="mode-hint">Parakeet Ultra. Audio and text stay on this Mac.</span></span></label>
    <label class="mode-option"><input type="radio" name="speech-engine" value="aws_transcribe"
      checked={cloud}
      onchange={() => settingsStore.update('speech_engine', 'aws_transcribe')} />
      <span><strong>AWS Transcribe</strong><span class="mode-hint">A cloud service of Amazon Web Services. Uses less memory. Needs an AWS account and an internet connection.</span></span></label>
  </div>
  {#if cloud}
    <div class="aws-options">
      <label class="setting-field">
        <span class="field-label">AWS profile</span>
        <select class="setting-select" value={aws.profile ?? ''} onchange={(event) => updateAws({ profile: event.currentTarget.value || null })}>
          {#if !aws.profile}<option value="">Choose a profile</option>{/if}
          {#each options?.profiles ?? [] as item (item.name)}
            <option value={item.name}>{item.name}</option>
          {/each}
          {#if aws.profile && !profile}
            <option value={aws.profile}>{options ? `${aws.profile} (not found)` : aws.profile}</option>
          {/if}
        </select>
      </label>
      {#if options?.profiles_error}
        <p class="setting-error" role="alert">{options.profiles_error}</p>
      {:else if options && !options.profiles.length}
        <p class="setting-hint">No AWS profile was found in ~/.aws/config or ~/.aws/credentials.</p>
      {/if}
      {#if optionsError}
        <div class="setting-error" role="alert">
          <p>{optionsError}</p>
          <button class="secondary-button" type="button" onclick={() => refreshOptions()}>Try again</button>
        </div>
      {/if}
      <label class="setting-field">
        <span class="field-label">Region</span>
        <select class="setting-select" value={aws.region ?? ''} onchange={(event) => updateAws({ region: event.currentTarget.value || null })}>
          <option value="">{profileRegion ? `From the profile (${profileRegion})` : 'Choose a Region'}</option>
          {#each options?.regions ?? [] as region (region.code)}
            <option value={region.code}>{region.name} ({region.code})</option>
          {/each}
          {#if aws.region && !options?.regions.some((region) => region.code === aws.region)}
            <option value={aws.region}>{aws.region}</option>
          {/if}
        </select>
      </label>
      {#if profile && !profile.region && !aws.region}<p class="setting-hint">This profile has no Region. Choose one.</p>{/if}
      <label class="setting-field">
        <span class="field-label">Language</span>
        <select class="setting-select" value={aws.language} onchange={(event) => updateAws({ language: event.currentTarget.value })}>
          {#each languages as language (language.code)}
            <option value={language.code}>{language.label}</option>
          {/each}
          {#if !languages.some((language) => language.code === aws.language)}
            <option value={aws.language}>{aws.language}</option>
          {/if}
        </select>
      </label>
      <label class="setting-field">
        <span class="field-label">Custom vocabulary</span>
        <input type="text" value={aws.vocabulary_name ?? ''} maxlength="200" spellcheck="false" placeholder="Optional" aria-describedby="aws-vocabulary-help"
          oninput={(event) => updateAws({ vocabulary_name: event.currentTarget.value || null })} />
      </label>
      <p class="setting-hint" id="aws-vocabulary-help">The name of a custom vocabulary in your AWS account, for the same Region and language.</p>
      <button class="secondary-button" type="button" disabled={testing || !aws.profile || !(aws.region || profileRegion)} onclick={testConnection}>{testing ? 'Testing…' : 'Test connection'}</button>
      {#if testResult?.ok}
        <p class="setting-success" role="status">{testResult.text}</p>
      {:else if testResult}
        <p class="setting-error" role="alert">{testResult.text}</p>
      {/if}
      <p class="setting-hint">SottoASR sends the audio of each completed dictation to Amazon Transcribe, in this Region, with your AWS account. AWS bills your account for each second of audio. By default, AWS can keep this audio to improve its services. A cancelled recording is not sent.</p>
      <p class="setting-hint aws-links">
        <a href={DATA_USE_URL} onclick={(event) => openLink(event, DATA_USE_URL)}>How AWS uses your audio</a> ·
        <a href={PRICING_URL} onclick={(event) => openLink(event, PRICING_URL)}>Pricing</a>
      </p>
      {#if linkError}<p class="setting-error" role="alert">{linkError}</p>{/if}
    </div>
  {/if}
</section>

<style>
  .aws-options { border-top:1px solid var(--border); margin-top:16px; padding-top:16px; }
  .aws-options input { display:block; margin-top:6px; width:100%; box-sizing:border-box; border:1px solid var(--border); background:var(--input-bg); color:var(--text-bright); padding:8px 10px; border-radius:8px; font:inherit; font-size:13px; }
  .aws-options input:focus { outline:1px solid var(--accent); }
  .aws-links a { color:var(--accent-text); }
</style>
