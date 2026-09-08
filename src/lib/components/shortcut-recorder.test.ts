import { afterEach, expect, it, vi } from 'vitest';
import { flushSync, mount, unmount } from 'svelte';
vi.mock('@tauri-apps/api/core',()=>({invoke:vi.fn(async()=>undefined)}));
vi.mock('@tauri-apps/api/event',()=>({listen:vi.fn(async()=>vi.fn())}));
import { invoke } from '@tauri-apps/api/core';
import ShortcutRecorder from './shortcut-recorder.svelte';
let component:ReturnType<typeof mount>|undefined;let target:HTMLDivElement;
afterEach(async()=>{if(component)await unmount(component);component=undefined;target?.remove();});
it('ends shortcut capture when its window loses focus',async()=>{
  const onchange=vi.fn();const onrecordend=vi.fn();target=document.createElement('div');document.body.append(target);
  component=mount(ShortcutRecorder,{target,props:{value:'F8',onchange,onrecordend}});flushSync();
  target.querySelector<HTMLButtonElement>('.shortcut-recorder')!.click();
  await vi.waitFor(()=>expect(invoke).toHaveBeenCalledWith('start_key_capture'));
  window.dispatchEvent(new Event('blur'));flushSync();
  expect(invoke).toHaveBeenCalledWith('stop_key_capture');expect(onrecordend).toHaveBeenCalledOnce();expect(onchange).not.toHaveBeenCalled();
});
