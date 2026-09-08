import { afterEach, expect, it, vi } from 'vitest';
import { flushSync, mount, unmount } from 'svelte';
import RecordingTimer from './recording-timer.svelte';
let component:ReturnType<typeof mount>|undefined;
let target:HTMLDivElement;
afterEach(async()=>{if(component)await unmount(component);component=undefined;target?.remove();vi.useRealTimers();});
it('recovers elapsed time from the native start timestamp and clears its interval',async()=>{
  vi.useFakeTimers();vi.setSystemTime(100_000);
  const props=$state({running:true,startedAt:87_655});
  target=document.createElement('div');document.body.append(target);component=mount(RecordingTimer,{target,props});flushSync();
  expect(target.textContent).toBe('0:12');vi.advanceTimersByTime(1750);flushSync();expect(target.textContent).toBe('0:14');
  props.running=false;flushSync();expect(target.textContent).toBe('0:00');expect(vi.getTimerCount()).toBe(0);
});
