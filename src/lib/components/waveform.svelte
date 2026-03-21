<script lang="ts">
  import { onMount, onDestroy } from 'svelte';

  interface Props {
    levels?: number[];
  }

  let { levels = [] }: Props = $props();

  let canvas: HTMLCanvasElement;
  let animFrameId: number;

  // Ring buffer for 5 seconds of data at ~30fps = 150 entries
  const BUFFER_SIZE = 150;
  const BAR_COUNT = 50;
  let ringBuffer = new Float32Array(BUFFER_SIZE);
  let writeIndex = 0;
  let sampleCount = 0;

  // Dynamic range: track rolling min/max
  let rollingMax = 0.005; // floor so we don't amplify pure silence

  // Clear ring buffer and canvas — called imperatively via window.__resetOverlay
  export function reset() {
    ringBuffer.fill(0);
    writeIndex = 0;
    sampleCount = 0;
    rollingMax = 0.005;
    // Immediately clear the canvas so no stale frame is visible
    if (canvas) {
      const ctx = canvas.getContext('2d');
      if (ctx) {
        ctx.clearRect(0, 0, canvas.width, canvas.height);
      }
    }
  }

  // Accept new levels from parent (pushed from Tauri events)
  $effect(() => {
    if (levels.length === 0) {
      // Clear ring buffer when levels are reset (new recording started)
      ringBuffer.fill(0);
      writeIndex = 0;
      sampleCount = 0;
      rollingMax = 0.005;
      return;
    }

    const latest = levels[levels.length - 1];
    ringBuffer[writeIndex] = latest;
    writeIndex = (writeIndex + 1) % BUFFER_SIZE;
    sampleCount++;

    // Update rolling max from the entire buffer
    let max = 0.005; // floor
    for (let i = 0; i < BUFFER_SIZE; i++) {
      if (ringBuffer[i] > max) max = ringBuffer[i];
    }
    // Smooth the max — decay slowly so it doesn't jump
    rollingMax = rollingMax * 0.95 + max * 0.05;
    if (rollingMax < 0.005) rollingMax = 0.005;
  });

  function render() {
    if (!canvas) {
      animFrameId = requestAnimationFrame(render);
      return;
    }

    const ctx = canvas.getContext('2d');
    if (!ctx) return;

    const dpr = window.devicePixelRatio || 1;
    const logicalWidth = canvas.clientWidth;
    const logicalHeight = canvas.clientHeight;

    // Set canvas backing store for HiDPI
    if (canvas.width !== Math.floor(logicalWidth * dpr) ||
        canvas.height !== Math.floor(logicalHeight * dpr)) {
      canvas.width = Math.floor(logicalWidth * dpr);
      canvas.height = Math.floor(logicalHeight * dpr);
      ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    }

    // Clear
    ctx.clearRect(0, 0, logicalWidth, logicalHeight);

    // Draw bars
    const barGap = 2;
    const barWidth = Math.max(2, (logicalWidth - (BAR_COUNT - 1) * barGap) / BAR_COUNT);
    const totalBarWidth = barWidth + barGap;
    const centerY = logicalHeight / 2;
    const maxBarHeight = logicalHeight - 2; // 1px padding top+bottom
    const minBarHeight = 3;

    for (let i = 0; i < BAR_COUNT; i++) {
      // Read from ring buffer: most recent data on the right
      // Map BAR_COUNT bars across the BUFFER_SIZE samples
      const bufferIndex = (writeIndex - BAR_COUNT + i + BUFFER_SIZE) % BUFFER_SIZE;
      const rawLevel = ringBuffer[bufferIndex];

      // Normalize against rolling max for dynamic range
      const normalized = Math.min(1, rawLevel / rollingMax);

      // Apply sqrt curve for perceptual scaling
      const scaled = Math.sqrt(normalized);

      const barHeight = Math.max(minBarHeight, scaled * maxBarHeight);
      const x = Math.round(i * totalBarWidth);
      const y = Math.round(centerY - barHeight / 2);

      // Opacity based on level
      const alpha = 0.3 + scaled * 0.7;
      ctx.fillStyle = `rgba(255, 255, 255, ${alpha})`;

      // Rounded rect (approximate with fillRect for speed)
      const radius = Math.min(barWidth / 2, 2);
      ctx.beginPath();
      ctx.roundRect(x, y, Math.round(barWidth), Math.round(barHeight), radius);
      ctx.fill();
    }

    animFrameId = requestAnimationFrame(render);
  }

  onMount(() => {
    animFrameId = requestAnimationFrame(render);
  });

  onDestroy(() => {
    if (animFrameId) cancelAnimationFrame(animFrameId);
  });
</script>

<canvas
  bind:this={canvas}
  class="waveform-canvas"
  aria-hidden="true"
></canvas>

<style>
  .waveform-canvas {
    width: 100%;
    height: 28px;
    display: block;
  }
</style>
