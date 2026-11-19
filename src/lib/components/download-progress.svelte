<script lang="ts">
  import type { DownloadProgress } from '../utils/tauri';

  let { progress, label }: { progress: DownloadProgress; label: string } = $props();
  const MB = 1_000_000;
  let known = $derived(progress.total_bytes > 0);
  let percent = $derived(known ? Math.min(100, Math.round((progress.downloaded_bytes / progress.total_bytes) * 100)) : 0);
  let downloaded = $derived(Math.round(progress.downloaded_bytes / MB).toLocaleString());
  let total = $derived(Math.round(progress.total_bytes / MB).toLocaleString());
</script>

<div class="download-progress">
  <div class="track" role="progressbar" aria-label={label} aria-valuemin={0} aria-valuemax={100} aria-valuenow={known ? percent : undefined}>
    <span class="fill" style:width={`${percent}%`}></span>
  </div>
  <p>{known ? `${percent}% · ${downloaded} of ${total} MB` : `${downloaded} MB`}</p>
</div>

<style>
  .download-progress { width: 100%; margin-top: 10px; }
  .track { height: 6px; border-radius: 3px; background: var(--border); overflow: hidden; }
  .fill { display: block; height: 100%; border-radius: 3px; background: var(--accent); transition: width 0.3s ease; }
  p { margin: 6px 0 0; font-size: 12px; color: var(--text-dim); font-variant-numeric: tabular-nums; }
</style>
