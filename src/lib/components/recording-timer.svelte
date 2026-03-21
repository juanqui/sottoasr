<script lang="ts">
  import { formatDuration } from '../utils/format';

  interface Props {
    running: boolean;
  }

  let { running }: Props = $props();

  let elapsed: number = $state(0);
  let frameId: number | null = $state(null);
  // Captured internally when running becomes true — no external dependency
  let internalStart: number | null = null;

  function tick() {
    if (internalStart != null) {
      elapsed = Date.now() - internalStart;
    }
    frameId = requestAnimationFrame(tick);
  }

  $effect(() => {
    if (running) {
      // Snapshot the start time the moment running becomes true
      internalStart = Date.now();
      elapsed = 0;
      frameId = requestAnimationFrame(tick);
    } else {
      internalStart = null;
      if (frameId != null) {
        cancelAnimationFrame(frameId);
        frameId = null;
      }
      elapsed = 0;
    }

    return () => {
      if (frameId != null) {
        cancelAnimationFrame(frameId);
        frameId = null;
      }
    };
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
