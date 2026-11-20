<script lang="ts">
  import { onMount } from 'svelte';
  import { normalizedAwsBedrock, settingsStore } from '../stores/settings.svelte';
  import { getBedrockCleanupOptions, openUrl, testBedrockCleanup } from '../utils/tauri';
  import type { AwsBedrockSettings, BedrockCleanupOptions, BedrockModel } from '../utils/tauri';
  import type { CleanupSetup } from '../stores/cleanup-setup.svelte';

  let { cleanup }: { cleanup: CleanupSetup } = $props();
  const DATA_PROTECTION_URL = 'https://docs.aws.amazon.com/bedrock/latest/userguide/data-protection.html';
  const PRICING_URL = 'https://aws.amazon.com/bedrock/pricing/';
  let options = $state<BedrockCleanupOptions | null>(null);
  let optionsError = $state('');
  let optionsRequest = 0;
  let testing = $state(false);
  let testResult = $state<{ ok: boolean; text: string } | null>(null);
  let testRequest = 0;
  let linkError = $state('');
  // While "On this Mac" prepares the local model, the draft keeps AWS Bedrock. The window shows the choice of the user.
  let cloud = $derived(settingsStore.current.cleanup_engine === 'aws_bedrock' && !cleanup.selectingLocal);
  let aws = $derived(settingsStore.current.aws_bedrock);
  let profile = $derived(options?.profiles.find((item) => item.name === aws.profile));
  let profileRegion = $derived(options?.regions.find((region) => region.code === profile?.region));
  // The Region of the settings, else the Region of the profile. Only a Region of the list has routes.
  let region = $derived(options?.regions.find((item) => item.code === (aws.region || profile?.region)));
  let route = $derived(region?.routes.find((item) => item.model === aws.model));

  async function refreshOptions() {
    const request = ++optionsRequest;
    try {
      const listed = await getBedrockCleanupOptions();
      if (request === optionsRequest) { options = listed; optionsError = ''; }
    } catch (error) {
      if (request === optionsRequest) optionsError = String(error);
    }
  }

  /** With cleanup on, the local model must be ready before the draft changes to "On this Mac". */
  function selectLocal() {
    if (!settingsStore.current.llm_cleanup_enabled || cleanup.status?.loaded === true) {
      settingsStore.update('cleanup_engine', 'local');
      return;
    }
    // The newest choice replaces a preparation in flight, such as a repair.
    cleanup.cancel();
    void cleanup.selectLocal(() => settingsStore.update('cleanup_engine', 'local'));
  }

  function selectAws() {
    cleanup.cancel();
    settingsStore.update('cleanup_engine', 'aws_bedrock');
  }

  /** Write one AWS option to the draft. A result of the connection test no longer applies. */
  function updateAws(change: Partial<AwsBedrockSettings>) {
    settingsStore.update('aws_bedrock', { ...settingsStore.current.aws_bedrock, ...change });
    ++testRequest;
    testResult = null;
  }

  async function testConnection() {
    const request = ++testRequest;
    testing = true;
    testResult = null;
    try {
      const result = await testBedrockCleanup(normalizedAwsBedrock(settingsStore.current.aws_bedrock));
      if (request === testRequest) testResult = { ok: true, text: `Amazon Bedrock answered in ${result.seconds.toFixed(1)} s (${result.model}, ${result.region}).` };
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

  // The AWS files are read only while AWS Bedrock is selected.
  $effect(() => { if (cloud && !options && !optionsError) void refreshOptions(); });

  onMount(() => {
    const refresh = () => { if (cloud) void refreshOptions(); };
    window.addEventListener('focus', refresh);
    return () => { ++optionsRequest; ++testRequest; window.removeEventListener('focus', refresh); };
  });
</script>

<div class="cleanup-engine">
  <span class="field-label" id="cleanup-engine-label">Cleanup engine</span>
  <div class="mode-options" role="radiogroup" aria-labelledby="cleanup-engine-label">
    <label class="mode-option"><input type="radio" name="cleanup-engine" value="local"
      checked={!cloud}
      onchange={selectLocal} />
      <span><strong>On this Mac</strong><span class="mode-hint">MiniCPM5 2B. The text stays on this Mac.</span></span></label>
    <label class="mode-option"><input type="radio" name="cleanup-engine" value="aws_bedrock"
      checked={cloud}
      onchange={selectAws} />
      <span><strong>AWS Bedrock</strong><span class="mode-hint">A cloud service of Amazon Web Services. No cleanup model is in memory. Needs an AWS account and an internet connection.</span></span></label>
  </div>
  {#if cloud && cleanup.selectError}<p class="setting-error" role="alert">{cleanup.selectError}</p>{/if}
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
          <option value="">{profileRegion ? `From the profile (${profileRegion.code})` : 'Choose a Region'}</option>
          {#each options?.regions ?? [] as item (item.code)}
            <option value={item.code}>{item.name} ({item.code})</option>
          {/each}
          {#if aws.region && !options?.regions.some((item) => item.code === aws.region)}
            <option value={aws.region}>{aws.region}</option>
          {/if}
        </select>
      </label>
      <label class="setting-field">
        <span class="field-label">Model</span>
        <select class="setting-select" value={aws.model} onchange={(event) => updateAws({ model: event.currentTarget.value as BedrockModel })}>
          {#each options?.models ?? [] as model (model.id)}
            <option value={model.id}>{model.name}</option>
          {/each}
          {#if !options?.models.some((model) => model.id === aws.model)}
            <option value={aws.model}>{aws.model}</option>
          {/if}
        </select>
      </label>
      {#if route}<p class="setting-hint route">Inference profile {route.inference_profile}. {route.processed_in}</p>{/if}
      <button class="secondary-button" type="button" disabled={testing || !aws.profile || !region} onclick={testConnection}>{testing ? 'Testing…' : 'Test connection'}</button>
      {#if testResult?.ok}
        <p class="setting-success" role="status">{testResult.text}</p>
      {:else if testResult}
        <p class="setting-error" role="alert">{testResult.text}</p>
      {/if}
      <p class="setting-hint">SottoASR sends the text of a dictation to Amazon Bedrock, with your AWS account, when the text has something to clean. AWS bills your account for the tokens. Amazon Bedrock does not give the text to the model provider. SottoASR examines each change that the model proposes, and it can only remove words.{aws.model === 'gpt-6.2-sol' ? ' For this model, AWS can keep text that its abuse classifier flags for up to 30 days.' : ''}</p>
      <p class="setting-hint aws-links">
        <a href={DATA_PROTECTION_URL} onclick={(event) => openLink(event, DATA_PROTECTION_URL)}>How Amazon Bedrock protects your data</a> ·
        <a href={PRICING_URL} onclick={(event) => openLink(event, PRICING_URL)}>Pricing</a>
      </p>
      {#if linkError}<p class="setting-error" role="alert">{linkError}</p>{/if}
    </div>
  {/if}
</div>

<style>
  .cleanup-engine { margin-top:14px; }
  .cleanup-engine .mode-options { margin-top:8px; }
  .aws-options { border-top:1px solid var(--border); margin-top:16px; padding-top:16px; }
  .aws-options > .secondary-button { margin-top:8px; }
  .route { overflow-wrap:anywhere; }
  .aws-links a { color:var(--accent-text); }
</style>
