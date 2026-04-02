<script lang="ts">
  import { getVersion } from '@tauri-apps/api/app';
  import { listen } from '@tauri-apps/api/event';
  import { onMount } from 'svelte';
  import { getUpdateStatus, checkAppUpdate, performAppUpdate } from '../utils/tauri';
  import type { UpdateStatus } from '../utils/tauri';
  import appIcon from '../../assets/app-icon.png';

  let version = $state('');
  getVersion().then(v => version = v);

  let updateStatus: UpdateStatus | null = $state(null);
  let checking = $state(false);
  let downloading = $state(false);
  let updateError = $state('');

  onMount(() => {
    const cleanups: Array<() => void> = [];
    // Load update status
    getUpdateStatus().then(s => updateStatus = s).catch(() => {});
    // Listen for update-available events
    listen<string>('update-available', () => {
      getUpdateStatus().then(s => updateStatus = s).catch(() => {});
    }).then(u => cleanups.push(u));
    return () => cleanups.forEach(fn => fn());
  });

  async function handleCheckUpdate() {
    checking = true;
    updateError = '';
    try {
      await checkAppUpdate();
      updateStatus = await getUpdateStatus();
    } catch (err: any) {
      updateError = err?.toString() || 'Check failed';
    } finally {
      checking = false;
    }
  }

  async function handleDownloadUpdate() {
    downloading = true;
    updateError = '';
    try {
      await performAppUpdate();
      updateStatus = await getUpdateStatus();
    } catch (err: any) {
      updateError = err?.toString() || 'Update failed';
    } finally {
      downloading = false;
    }
  }

  const sections = [
    {
      title: 'Speech Recognition',
      items: [
        { name: 'NVIDIA Parakeet TDT v3', desc: 'ASR model, 600M params, 25 languages', license: 'CC-BY-4.0' },
        { name: 'FluidAudio', desc: 'CoreML/ANE inference engine (Swift)', license: 'Apache-2.0' },
        { name: 'parakeet-rs', desc: 'ONNX Runtime Rust bindings', license: 'MIT' },
        { name: 'cpal', desc: 'Cross-platform audio capture', license: 'Apache-2.0' },
        { name: 'hound', desc: 'WAV encoding/decoding', license: 'Apache-2.0' },
        { name: 'rubato', desc: 'Audio resampling', license: 'MIT' },
      ],
    },
    {
      title: 'AI Transcript Cleanup',
      items: [
        { name: 'Qwen3.5-0.8B', desc: 'LLM by Alibaba Cloud (Qwen team)', license: 'Apache-2.0' },
        { name: 'Apple MLX', desc: 'Metal-native ML framework', license: 'MIT' },
        { name: 'mlx-lm', desc: 'MLX language model inference', license: 'MIT' },
        { name: 'huggingface_hub', desc: 'Model download and caching', license: 'Apache-2.0' },
      ],
    },
    {
      title: 'Application Framework',
      items: [
        { name: 'Tauri v2', desc: 'Native app shell (Rust + Web)', license: 'MIT' },
        { name: 'Svelte 5', desc: 'Reactive UI framework', license: 'MIT' },
        { name: 'tauri-nspanel', desc: 'macOS NSPanel overlay windows', license: 'MIT' },
        { name: 'Vite', desc: 'Frontend build tool', license: 'MIT' },
        { name: 'Tokio', desc: 'Async Rust runtime', license: 'MIT' },
        { name: 'serde', desc: 'Serialization framework', license: 'MIT' },
      ],
    },
  ];
</script>

<div class="about-scroll">
  <!-- Hero -->
  <div class="hero">
    <img class="icon" src={appIcon} alt="SottoASR" />
    <h1>SottoASR</h1>
    <p class="version">Version {version}</p>

    <!-- Update status -->
    <div class="update-section">
      {#if updateStatus?.restart_pending}
        <span class="update-badge restart">Update installed — restart to apply</span>
      {:else if downloading || updateStatus?.downloading}
        <div class="update-downloading">
          <div class="spinner-small"></div>
          <span>Downloading update...</span>
        </div>
      {:else if updateStatus?.update_available && updateStatus?.version}
        <span class="update-badge available">v{updateStatus.version} available</span>
        <button class="update-action-btn" onclick={handleDownloadUpdate} type="button">
          Download & Install
        </button>
      {:else}
        <button class="check-update-btn" onclick={handleCheckUpdate} disabled={checking} type="button">
          {checking ? 'Checking...' : 'Check for Updates'}
        </button>
      {/if}
      {#if updateError}
        <p class="update-error">{updateError}</p>
      {/if}
    </div>

    <p class="tagline">Local, privacy-first speech-to-text for macOS</p>
    <p class="detail">
      All processing happens on-device.<br />
      No audio or text is ever sent to a cloud service.
    </p>
  </div>

  <hr class="sep" />

  <!-- Acknowledgements -->
  <p class="section-heading">Acknowledgements</p>

  {#each sections as section}
    <p class="group-heading">{section.title}</p>
    {#each section.items as item}
      <div class="row">
        <div class="row-left">
          <span class="row-name">{item.name}</span>
          <span class="row-desc">{item.desc}</span>
        </div>
        <span class="row-badge">{item.license}</span>
      </div>
    {/each}
  {/each}

  <hr class="sep" />

  <!-- License -->
  <div class="footer">
    <p>SottoASR is open source under the <strong>MIT License</strong></p>
    <p class="footer-note">
      All 660+ dependencies use permissive or weak-copyleft licenses
      (MIT, Apache-2.0, BSD, MPL-2.0, Unicode-3.0, ISC, Zlib, CC-BY-4.0).
      See THIRD_PARTY_LICENSES for the full list.
    </p>
    <p class="footer-copy">&copy; 2026 Juan Villa</p>
  </div>

  <hr class="sep" />

  <!-- Contributors -->
  <p class="section-heading">Contributors</p>
  <div class="contributors">
    <p class="contributor-name">Ian Scofield</p>
    <p class="contributor-name">Young Park</p>
  </div>
</div>

<style>
  .about-scroll {
    height: 100vh;
    overflow-y: auto;
    padding: 28px 32px 24px;
    box-sizing: border-box;
    user-select: none;
  }

  /* ---- Hero ---- */
  .hero {
    text-align: center;
  }

  .icon {
    width: 80px;
    height: 80px;
    border-radius: 18px;
    margin-bottom: 12px;
  }

  h1 {
    font-size: 21px;
    font-weight: 600;
    color: var(--text-bright);
    margin: 0 0 2px;
  }

  .version {
    font-size: 12px;
    color: var(--text-dim);
    margin: 0 0 14px;
  }

  .tagline {
    font-size: 13px;
    font-weight: 500;
    color: var(--text);
    margin: 0 0 6px;
  }

  .detail {
    font-size: 11px;
    color: var(--text-dim);
    line-height: 1.6;
    margin: 0;
  }

  /* ---- Update section ---- */
  .update-section {
    margin: 8px 0 10px;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 6px;
  }

  .update-badge {
    font-size: 11px;
    font-weight: 500;
    padding: 3px 10px;
    border-radius: 8px;
  }

  .update-badge.available {
    background: rgba(59, 130, 246, 0.12);
    color: rgba(59, 130, 246, 0.9);
  }

  .update-badge.restart {
    background: rgba(34, 197, 94, 0.12);
    color: #22c55e;
  }

  .update-action-btn {
    padding: 4px 12px;
    border-radius: 6px;
    border: 1px solid rgba(59, 130, 246, 0.5);
    background: rgba(59, 130, 246, 0.1);
    color: rgba(59, 130, 246, 0.9);
    font-size: 11px;
    font-family: inherit;
    cursor: pointer;
  }

  .update-action-btn:hover {
    background: rgba(59, 130, 246, 0.2);
  }

  .check-update-btn {
    padding: 4px 12px;
    border-radius: 6px;
    border: 1px solid var(--border);
    background: none;
    color: var(--text-dim);
    font-size: 11px;
    font-family: inherit;
    cursor: pointer;
  }

  .check-update-btn:hover:not(:disabled) {
    border-color: var(--border-hover);
    color: var(--text);
  }

  .check-update-btn:disabled {
    opacity: 0.5;
    cursor: default;
  }

  .update-downloading {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 11px;
    color: var(--text-dim);
  }

  .update-downloading .spinner-small {
    width: 12px;
    height: 12px;
    border-radius: 50%;
    border: 2px solid var(--border);
    border-top-color: var(--accent);
    animation: spin 0.8s linear infinite;
  }

  @keyframes spin {
    to { transform: rotate(360deg); }
  }

  .update-error {
    font-size: 10px;
    color: #ef4444;
    margin: 0;
  }

  /* ---- Separator ---- */
  .sep {
    border: none;
    border-top: 1px solid var(--border);
    margin: 18px 0;
  }

  /* ---- Section / group headings ---- */
  .section-heading {
    font-size: 10px;
    font-weight: 700;
    text-transform: uppercase;
    letter-spacing: 1.2px;
    color: var(--text-dim);
    text-align: center;
    margin: 0 0 14px;
  }

  .group-heading {
    font-size: 10px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.6px;
    color: var(--text-dim);
    margin: 12px 0 6px;
    padding-bottom: 4px;
    border-bottom: 1px solid var(--border);
  }

  /* ---- Rows ---- */
  .row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    padding: 6px 0;
  }

  .row-left {
    display: flex;
    flex-direction: column;
    gap: 1px;
    min-width: 0;
  }

  .row-name {
    font-size: 12px;
    font-weight: 500;
    color: var(--text);
  }

  .row-desc {
    font-size: 10px;
    color: var(--text-dim);
    line-height: 1.3;
  }

  .row-badge {
    flex-shrink: 0;
    font-size: 9px;
    font-weight: 500;
    padding: 2px 7px;
    border-radius: 6px;
    background: rgba(255, 255, 255, 0.06);
    color: var(--text-dim);
    white-space: nowrap;
  }

  /* ---- Footer ---- */
  .footer {
    text-align: center;
  }

  .footer p {
    margin: 0 0 6px;
    font-size: 11px;
    color: var(--text);
  }

  .footer-note {
    font-size: 10px !important;
    color: var(--text-dim) !important;
    line-height: 1.5;
    margin-bottom: 10px !important;
  }

  .footer-copy {
    font-size: 10px !important;
    color: var(--text-dim) !important;
    opacity: 0.5;
  }

  /* ---- Contributors ---- */
  .contributors {
    text-align: center;
    margin-bottom: 16px;
  }

  .contributor-name {
    font-size: 12px;
    color: var(--text);
    margin: 4px 0;
  }
</style>
