<script lang="ts">
  import { formatDuration } from '../utils/format';

  interface Props {
    running: boolean;
    startedAt?: number;
  }

  let { running, startedAt }: Props = $props();

  let elapsed = $state(0);

  $effect(() => {
    elapsed = 0;
    if (!running) return;
    const started = startedAt ?? Date.now();
    elapsed = Math.max(0, Date.now() - started);
    // The display has second precision. Wall-clock time avoids interval drift.
    const interval = setInterval(() => { elapsed = Math.max(0, Date.now() - started); }, 250);
    return () => clearInterval(interval);
  });

  let display: string = $derived(formatDuration(elapsed));
</script>

<span class="timer">{display}</span>

<style>
  .timer {
    font-family: ui-monospace, 'SF Mono', Consolas, monospace;
    font-size: 13px;
    font-weight: 500;
    color: rgba(255, 255, 255, 0.8);
    letter-spacing: 0.5px;
    min-width: 36px;
    text-align: right;
    flex-shrink: 0;
    user-select: none;
  }
</style>
