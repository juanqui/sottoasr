import { afterEach, expect, it, vi } from 'vitest';
import { flushSync, mount, unmount } from 'svelte';
vi.mock('@tauri-apps/api/app',()=>({getVersion:vi.fn(async()=> '0.8.0')}));
vi.mock('@tauri-apps/api/core',()=>({invoke:vi.fn()}));
vi.mock('@tauri-apps/api/window',()=>({getCurrentWindow:()=>({close:vi.fn()})}));
const {listeners}=vi.hoisted(()=>({listeners:new Map<string,(event:{payload:unknown})=>void>()}));
vi.mock('@tauri-apps/api/event',()=>({listen:vi.fn(async(name:string,callback:(event:{payload:unknown})=>void)=>{listeners.set(name,callback);return()=>listeners.delete(name);})}));
vi.mock('../utils/tauri',()=>({getUpdateStatus:vi.fn(async()=>({})),checkAppUpdate:vi.fn(),performAppUpdate:vi.fn(),updateLlmModel:vi.fn()}));
import { checkAppUpdate, getUpdateStatus, updateLlmModel } from '../utils/tauri';
import UpdateView from './update-view.svelte';
import { invoke } from '@tauri-apps/api/core';
let component:ReturnType<typeof mount>|undefined;let target:HTMLDivElement;
afterEach(async()=>{if(component)await unmount(component);component=undefined;target?.remove();});
it('does not start an autoclose timer when a check completes after unmount',async()=>{
  let resolve!:(value:string|null)=>void;
  vi.mocked(checkAppUpdate).mockReturnValueOnce(new Promise((yes)=>{resolve=yes;}));
  target=document.createElement('div');document.body.append(target);component=mount(UpdateView,{target});flushSync();
  await vi.waitFor(()=>expect(checkAppUpdate).toHaveBeenCalledOnce());
  await unmount(component);component=undefined;
  const interval=vi.spyOn(globalThis,'setInterval');resolve(null);await Promise.resolve();await Promise.resolve();
  expect(interval).not.toHaveBeenCalled();
});

it('renders the backend object-shaped model download error',async()=>{
  vi.mocked(getUpdateStatus).mockResolvedValue({model_update_available:true} as Awaited<ReturnType<typeof getUpdateStatus>>);
  let reject!:(error:Error)=>void;vi.mocked(updateLlmModel).mockReturnValueOnce(new Promise((_,no)=>{reject=no;}));
  target=document.createElement('div');document.body.append(target);component=mount(UpdateView,{target});flushSync();
  await vi.waitFor(()=>expect(target.textContent).toContain('Update Model'));
  Array.from(target.querySelectorAll('button')).find((button)=>button.textContent?.includes('Update Model'))!.click();flushSync();
  listeners.get('llm-download-error')!({payload:{message:'Model download interrupted'}});flushSync();
  expect(target.textContent).toContain('Model download interrupted');expect(target.textContent).not.toContain('[object Object]');
  reject(new Error('Model download interrupted'));await Promise.resolve();
});

it('keeps restart available for retry after the backend refuses an active dictation',async()=>{
  vi.mocked(getUpdateStatus).mockResolvedValue({restart_pending:true} as Awaited<ReturnType<typeof getUpdateStatus>>);
  vi.mocked(invoke).mockRejectedValueOnce('Finish dictation before restarting.');
  target=document.createElement('div');document.body.append(target);component=mount(UpdateView,{target});flushSync();
  await vi.waitFor(()=>expect(target.textContent).toContain('Restart Now'));
  const restart=Array.from(target.querySelectorAll('button')).find((button)=>button.textContent?.trim()==='Restart Now')!;
  restart.click();await vi.waitFor(()=>expect(target.textContent).toContain('Finish dictation before restarting.'));
  expect(invoke).toHaveBeenCalledWith('restart_app');expect(restart.disabled).toBe(false);
  vi.mocked(invoke).mockResolvedValueOnce(undefined);restart.click();await vi.waitFor(()=>expect(invoke).toHaveBeenCalledTimes(2));
});
