<script lang="ts">
  import { formatDuration } from '../utils/format';

  interface Props {
    startTime: number | null;
    running: boolean;
  }

  let { startTime, running }: Props = $props();

  let elapsed: number = $state(0);
  let frameId: number | null = $state(null);

  function tick() {
    if (startTime != null) {
      elapsed = Date.now() - startTime;
    }
    frameId = requestAnimationFrame(tick);
  }

  $effect(() => {
    if (running && startTime != null) {
      elapsed = Date.now() - startTime;
      frameId = requestAnimationFrame(tick);
    } else {
      if (frameId != null) {
        cancelAnimationFrame(frameId);
        frameId = null;
      }
      if (!running) {
        elapsed = 0;
      }
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
