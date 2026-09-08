import { beforeEach, afterEach, expect, it, vi } from 'vitest';
import { mount, unmount, flushSync } from 'svelte';
const { listeners } = vi.hoisted(() => ({ listeners:new Map<string, () => void>() }));
vi.mock('@tauri-apps/api/event', () => ({ listen:vi.fn(async (event:string, callback:()=>void) => {listeners.set(event,callback);return () => listeners.delete(event);}) }));
vi.mock('../utils/tauri', () => ({getModelStatus:vi.fn(),getLlmStatus:vi.fn(),initAsr:vi.fn(),prepareLlmModel:vi.fn()}));
import { getModelStatus } from '../utils/tauri';
import { CleanupSetup } from '../stores/cleanup-setup.svelte';
import ModelDashboard from './model-dashboard.svelte';
let target:HTMLDivElement;
let component:ReturnType<typeof mount>|undefined;
let cleanup:CleanupSetup;
beforeEach(() => {
  vi.clearAllMocks(); listeners.clear();
  vi.mocked(getModelStatus).mockResolvedValue({downloaded:true,loaded:true,initializing:false,error:null,path:null,name:'Parakeet v3',size_bytes:null});
  cleanup = new CleanupSetup();
  cleanup.status = {available:true,enabled:true,busy:false,loaded:true,downloaded:true,downloading:false,preparing:false,setup_error:null,unavailable_reason:null,model_name:'MiniCPM5',model_url:'',model_path:null,download_size_mb:1427,update_available:false,last_cleanup_status:{kind:'rejected',detail:{reason:'Protected span changed'}}};
});
afterEach(async () => {if(component) await unmount(component);component=undefined;cleanup.dispose();target?.remove();});
async function render() {target=document.createElement('div');document.body.append(target);component=mount(ModelDashboard,{target,props:{cleanup,onconfigure:vi.fn()}});flushSync();await vi.waitFor(()=>expect(target.textContent).toContain('Parakeet v3'));flushSync();}
it('shows loaded readiness separately from a rejected last proposal', async () => {
  await render();expect(target.querySelectorAll('.model-state.ready')).toHaveLength(2);
  expect(target.textContent).toContain('Last recording: Original kept');
  expect(target.textContent).toContain('The model responded');
});
it('never equates downloaded files or a disabled resident model with enabled readiness', async () => {
  await render(); cleanup.status={...cleanup.status!,loaded:false};flushSync();
  expect(target.querySelector('[aria-label="AI cleanup model"]')?.textContent).toContain('Not loaded');
  cleanup.status={...cleanup.status!,loaded:true,enabled:false};flushSync();
  expect(target.querySelector('[aria-label="AI cleanup model"] .ready')).toBeNull();
  expect(target.textContent).toContain('off in saved settings');
});
it('clears stale ASR readiness after a failed status read and removes listeners on close', async () => {
  await render();vi.mocked(getModelStatus).mockRejectedValueOnce(new Error('IPC disconnected'));
  listeners.get('asr-init-error')!();await vi.waitFor(()=>expect(target.textContent).toContain('IPC disconnected'));
  expect(target.querySelector('[aria-label="Speech recognition model"] .ready')).toBeNull();
  await unmount(component!);component=undefined;expect(listeners.size).toBe(0);
});
