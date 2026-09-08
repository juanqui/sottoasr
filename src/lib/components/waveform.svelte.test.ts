import { afterEach, expect, it, vi } from 'vitest';
import { flushSync, mount, unmount } from 'svelte';
import Waveform from './waveform.svelte';

let component: ReturnType<typeof mount> | undefined;
let target: HTMLDivElement;
afterEach(async () => {
  if (component) await unmount(component);
  component = undefined;
  target?.remove();
  vi.unstubAllGlobals();
});

function render(reduceMotion = false) {
  const frames = new Map<number, FrameRequestCallback>();
  let id = 0;
  const raf = vi.fn((callback: FrameRequestCallback) => { frames.set(++id, callback); return id; });
  const cancel = vi.fn((frame: number) => frames.delete(frame));
  vi.stubGlobal('requestAnimationFrame', raf);
  vi.stubGlobal('cancelAnimationFrame', cancel);
  vi.stubGlobal('ResizeObserver', class { observe() {} disconnect() {} });
  const media = Object.assign(new EventTarget(), { matches: reduceMotion });
  const removeMediaListener = vi.spyOn(media, 'removeEventListener');
  vi.stubGlobal('matchMedia', vi.fn(() => media));
  const ctx = { clearRect: vi.fn(), setTransform: vi.fn(), beginPath: vi.fn(), roundRect: vi.fn(), fill: vi.fn(), fillStyle: '' };
  vi.spyOn(HTMLCanvasElement.prototype, 'getContext').mockReturnValue(ctx as unknown as CanvasRenderingContext2D);
  const props = $state({ active: false, level: 0, sampleId: 0 });
  target = document.createElement('div'); document.body.append(target);
  component = mount(Waveform, { target, props }); flushSync();
  const draw = () => { const next = frames.entries().next().value!; frames.delete(next[0]); next[1](0); };
  return { props, frames, raf, cancel, ctx, media, removeMediaListener, draw };
}

it('draws only when active and a new sample or resize needs a frame', () => {
  const { props, frames, raf, cancel, ctx, draw } = render();
  expect(raf).not.toHaveBeenCalled();
  props.active = true; props.level = .01; props.sampleId = 1; flushSync(); expect(frames.size).toBe(1);
  draw(); expect(ctx.roundRect).toHaveBeenCalledTimes(50); expect(frames.size).toBe(0);
  props.sampleId = 2; flushSync(); expect(frames.size).toBe(1);
  props.active = false; flushSync(); expect(frames.size).toBe(0); expect(cancel).toHaveBeenCalled();
});

it('keeps a steady baseline under Reduce Motion and resumes live levels when the preference changes', async () => {
  const { props, frames, ctx, media, removeMediaListener, draw } = render(true);
  props.active = true; flushSync(); draw(); expect(ctx.roundRect).toHaveBeenCalledTimes(50);
  for (let sample = 1; sample <= 30; sample++) {
    props.level = sample / 100; props.sampleId = sample; flushSync();
  }
  expect(frames.size).toBe(0);
  media.matches = false; media.dispatchEvent(new Event('change')); flushSync();
  expect(frames.size).toBe(1); draw();
  props.sampleId++; flushSync(); expect(frames.size).toBe(1);
  media.matches = true; media.dispatchEvent(new Event('change')); flushSync(); draw();
  props.sampleId++; flushSync(); expect(frames.size).toBe(0);
  await unmount(component!); component = undefined;
  await vi.waitFor(() => expect(removeMediaListener.mock.calls[0]?.slice(0, 2)).toEqual(['change', expect.any(Function)]));
});
