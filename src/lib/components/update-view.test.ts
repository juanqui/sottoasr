import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { flushSync, mount, unmount } from 'svelte';
vi.mock('@tauri-apps/api/app',()=>({getVersion:vi.fn(async()=> '0.8.0')}));
vi.mock('@tauri-apps/api/window',()=>({getCurrentWindow:()=>({close:vi.fn()})}));
const {listeners}=vi.hoisted(()=>({listeners:new Map<string,(event:{payload:unknown})=>void>()}));
vi.mock('@tauri-apps/api/event',()=>({listen:vi.fn(async(name:string,callback:(event:{payload:unknown})=>void)=>{listeners.set(name,callback);return()=>listeners.delete(name);})}));
vi.mock('../utils/tauri',()=>({getUpdateStatus:vi.fn(),checkAppUpdate:vi.fn(),performAppUpdate:vi.fn(),restartApp:vi.fn()}));
import { checkAppUpdate, getUpdateStatus, restartApp } from '../utils/tauri';
import type { UpdateStatus } from '../utils/tauri';
import UpdateView from './update-view.svelte';
const idle:UpdateStatus={update_available:false,version:null,release_notes:null,downloading:false,restart_pending:false};
let component:ReturnType<typeof mount>|undefined;let target:HTMLDivElement;
beforeEach(()=>{vi.clearAllMocks();listeners.clear();vi.mocked(getUpdateStatus).mockResolvedValue({...idle});vi.mocked(checkAppUpdate).mockReturnValue(new Promise(()=>{}));});
afterEach(async()=>{if(component)await unmount(component);component=undefined;target?.remove();});
function render(){target=document.createElement('div');document.body.append(target);component=mount(UpdateView,{target});flushSync();}
it('does not start an autoclose timer when a check completes after unmount',async()=>{
  let resolve!:(value:string|null)=>void;
  vi.mocked(checkAppUpdate).mockReturnValueOnce(new Promise((yes)=>{resolve=yes;}));
  render();
  await vi.waitFor(()=>expect(checkAppUpdate).toHaveBeenCalledOnce());
  await unmount(component!);component=undefined;
  const interval=vi.spyOn(globalThis,'setInterval');resolve(null);await Promise.resolve();await Promise.resolve();
  expect(interval).not.toHaveBeenCalled();
});

it('ignores background check events unless it is checking',async()=>{
  vi.mocked(getUpdateStatus).mockResolvedValue({...idle,update_available:true,version:'0.9.0',release_notes:'Fixes'});
  render();
  await vi.waitFor(()=>expect(target.textContent).toContain('Update Available'));
  listeners.get('update-up-to-date')!({payload:null});
  listeners.get('update-check-error')!({payload:'Network unreachable'});
  listeners.get('update-available')!({payload:'0.9.1'});
  flushSync();
  expect(target.textContent).toContain('Version 0.9.0 is ready to download.');
  expect(target.textContent).not.toContain('Network unreachable');
  expect(getUpdateStatus).toHaveBeenCalledOnce();
  expect(checkAppUpdate).not.toHaveBeenCalled();
});

it('shows a check error that arrives while it is checking',async()=>{
  render();
  await vi.waitFor(()=>expect(checkAppUpdate).toHaveBeenCalledOnce());
  listeners.get('update-check-error')!({payload:'Network unreachable'});flushSync();
  expect(target.textContent).toContain('Update Failed');
  expect(target.textContent).toContain('Network unreachable');
});

it('opens on a download in progress and moves to ready when the install finishes',async()=>{
  vi.mocked(getUpdateStatus).mockResolvedValue({...idle,update_available:true,version:'0.9.1',downloading:true});
  render();
  await vi.waitFor(()=>expect(target.textContent).toContain('Downloading Update'));
  expect(target.textContent).toContain('v0.9.1');
  expect(checkAppUpdate).not.toHaveBeenCalled();
  listeners.get('update-installed')!({payload:{version:'0.9.1'}});flushSync();
  expect(target.textContent).toContain('Update Ready');
  expect(target.textContent).toContain('SottoASR v0.9.1 has been downloaded.');
});

it('keeps restart available for retry after the backend refuses an active dictation',async()=>{
  vi.mocked(getUpdateStatus).mockResolvedValue({...idle,restart_pending:true,version:'0.9.1'});
  vi.mocked(restartApp).mockRejectedValueOnce('Finish dictation before restarting.');
  render();
  await vi.waitFor(()=>expect(target.textContent).toContain('Restart Now'));
  expect(target.textContent).toContain('SottoASR v0.9.1');
  const restart=Array.from(target.querySelectorAll('button')).find((button)=>button.textContent?.trim()==='Restart Now')!;
  restart.click();await vi.waitFor(()=>expect(target.textContent).toContain('Finish dictation before restarting.'));
  expect(restartApp).toHaveBeenCalledOnce();expect(restart.disabled).toBe(false);
  vi.mocked(restartApp).mockResolvedValueOnce(undefined);restart.click();await vi.waitFor(()=>expect(restartApp).toHaveBeenCalledTimes(2));
});
