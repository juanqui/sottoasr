<script lang="ts">
  import { onMount } from 'svelte';
  import { MediaQuery } from 'svelte/reactivity';

  interface Props { level?: number; sampleId?: number; active?: boolean; }
  let { level = 0, sampleId = 0, active = false }: Props = $props();
  const reducedMotion = new MediaQuery('(prefers-reduced-motion: reduce)');
  let canvas: HTMLCanvasElement;
  let frame: number | null = null;
  let width = 0;
  let height = 0;
  let mounted = false;
  const BUFFER_SIZE = 150;
  const BAR_COUNT = 50;
  const ringBuffer = new Float32Array(BUFFER_SIZE);
  let writeIndex = 0;
  let rollingMax = 0.005;

  export function reset() {
    ringBuffer.fill(0);
    writeIndex = 0;
    rollingMax = 0.005;
    if (frame !== null) { cancelAnimationFrame(frame); frame = null; }
    canvas?.getContext('2d')?.clearRect(0, 0, canvas.width, canvas.height);
  }

  function scheduleDraw() {
    if (mounted && active && frame === null) frame = requestAnimationFrame(render);
  }

  $effect(() => {
    if (!active) {
      if (frame !== null) { cancelAnimationFrame(frame); frame = null; }
      return;
    }
    // Keep a steady waveform baseline under Reduce Motion. This branch does
    // not read sampleId/level, so incoming audio cannot schedule more frames.
    if (reducedMotion.current) { reset(); scheduleDraw(); return; }
    if (sampleId === 0) { reset(); scheduleDraw(); return; }
    ringBuffer[writeIndex] = Number.isFinite(level) ? Math.max(0, level) : 0;
    writeIndex = (writeIndex + 1) % BUFFER_SIZE;
    let max = 0.005;
    for (const sample of ringBuffer) max = Math.max(max, sample);
    rollingMax = Math.max(0.005, rollingMax * 0.95 + max * 0.05);
    scheduleDraw();
  });

  function render() {
    frame = null;
    if (!active || !mounted) return;
    const ctx = canvas.getContext('2d');
    if (!ctx) return;
    const dpr = window.devicePixelRatio || 1;
    if (canvas.width !== Math.floor(width * dpr) || canvas.height !== Math.floor(height * dpr)) {
      canvas.width = Math.floor(width * dpr);
      canvas.height = Math.floor(height * dpr);
      ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    }
    ctx.clearRect(0, 0, width, height);
    const gap = 2;
    const barWidth = Math.max(2, (width - (BAR_COUNT - 1) * gap) / BAR_COUNT);
    for (let i = 0; i < BAR_COUNT; i++) {
      const index = (writeIndex - BAR_COUNT + i + BUFFER_SIZE) % BUFFER_SIZE;
      const scaled = Math.sqrt(Math.min(1, ringBuffer[index] / rollingMax));
      const barHeight = Math.max(3, scaled * (height - 2));
      ctx.fillStyle = `rgba(255, 255, 255, ${0.3 + scaled * 0.7})`;
      ctx.beginPath();
      ctx.roundRect(Math.round(i * (barWidth + gap)), Math.round((height - barHeight) / 2), Math.round(barWidth), Math.round(barHeight), Math.min(barWidth / 2, 2));
      ctx.fill();
    }
  }

  onMount(() => {
    mounted = true;
    const resize = () => { width = canvas.clientWidth; height = canvas.clientHeight; scheduleDraw(); };
    const observer = new ResizeObserver(resize);
    observer.observe(canvas);
    resize();
    return () => { mounted = false; observer.disconnect(); if (frame !== null) cancelAnimationFrame(frame); };
  });
</script>
<canvas bind:this={canvas} class="waveform-canvas" aria-hidden="true"></canvas>
<style>
  .waveform-canvas { width:100%; height:28px; display:block; }
</style>
