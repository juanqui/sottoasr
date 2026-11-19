import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { flushSync, mount, unmount } from 'svelte';
vi.mock('../utils/tauri',()=>({startKeyCapture:vi.fn(async()=>undefined),stopKeyCapture:vi.fn(async()=>undefined)}));
vi.mock('@tauri-apps/api/event',()=>({listen:vi.fn(async()=>vi.fn())}));
import { startKeyCapture, stopKeyCapture } from '../utils/tauri';
import ShortcutRecorder from './shortcut-recorder.svelte';
let component:ReturnType<typeof mount>|undefined;let target:HTMLDivElement;
beforeEach(()=>{vi.clearAllMocks();});
afterEach(async()=>{if(component)await unmount(component);component=undefined;target?.remove();});
function render(props:Record<string,unknown>){target=document.createElement('div');document.body.append(target);component=mount(ShortcutRecorder,{target,props:{value:'F8',onchange:vi.fn(),...props}});flushSync();}
const press=(type:'keydown'|'keyup',init:KeyboardEventInit)=>{document.dispatchEvent(new KeyboardEvent(type,{bubbles:true,...init}));flushSync();};
it('ends shortcut capture when its window loses focus',async()=>{
  const onchange=vi.fn();const onrecordend=vi.fn();render({onchange,onrecordend});
  target.querySelector<HTMLButtonElement>('.shortcut-recorder')!.click();
  await vi.waitFor(()=>expect(startKeyCapture).toHaveBeenCalledOnce());
  window.dispatchEvent(new Event('blur'));flushSync();
  expect(stopKeyCapture).toHaveBeenCalledOnce();expect(onrecordend).toHaveBeenCalledOnce();expect(onchange).not.toHaveBeenCalled();
});

it('records Control together with Command',async()=>{
  const onchange=vi.fn();render({onchange});
  target.querySelector<HTMLButtonElement>('.shortcut-recorder')!.click();flushSync();
  press('keydown',{key:'k',code:'KeyK',metaKey:true,ctrlKey:true});
  press('keyup',{key:'Meta',code:'MetaLeft'});
  expect(onchange).toHaveBeenCalledWith('Control+CommandOrControl+KeyK');
});

it('offers Reset to default instead of Clear on a required shortcut, and ignores Backspace',async()=>{
  const onchange=vi.fn();render({value:'F8',defaultValue:'Escape',onchange,label:'Cancel recording shortcut'});
  expect(target.querySelector('.clear-btn')).toBeNull();
  target.querySelector<HTMLButtonElement>('.shortcut-recorder')!.click();flushSync();
  press('keydown',{key:'Backspace',code:'Backspace'});
  expect(onchange).not.toHaveBeenCalled();
  press('keydown',{key:'Escape',code:'Escape'});
  expect(onchange).not.toHaveBeenCalled();
  Array.from(target.querySelectorAll('button')).find((button)=>button.textContent?.trim()==='Reset to default')!.click();
  expect(onchange).toHaveBeenCalledWith('Escape');
});

it('keeps Clear on an optional shortcut',async()=>{
  const onchange=vi.fn();render({value:'F8',onchange,label:'Alternate cancel recording shortcut'});
  expect(target.textContent).not.toContain('Reset to default');
  target.querySelector<HTMLButtonElement>('.clear-btn')!.click();
  expect(onchange).toHaveBeenCalledWith('');
});
