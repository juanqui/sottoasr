import { beforeEach, afterEach, expect, it, vi } from 'vitest';
import { mount, unmount, flushSync } from 'svelte';
const { listeners } = vi.hoisted(() => ({ listeners:new Map<string, (event?:{payload:unknown}) => void>() }));
vi.mock('@tauri-apps/api/event', () => ({ listen:vi.fn(async (event:string, callback:(event?:{payload:unknown})=>void) => {listeners.set(event,callback);return () => listeners.delete(event);}) }));
vi.mock('../utils/tauri', () => ({getModelStatus:vi.fn(),getLlmStatus:vi.fn(),initAsr:vi.fn(),prepareLlmModel:vi.fn(),repairLlmRuntime:vi.fn(),redownloadSpeechModel:vi.fn(),deletePreviousSpeechModel:vi.fn()}));
import { deletePreviousSpeechModel, getModelStatus, redownloadSpeechModel, repairLlmRuntime } from '../utils/tauri';
import { CleanupSetup } from '../stores/cleanup-setup.svelte';
import ModelDashboard from './model-dashboard.svelte';
let target:HTMLDivElement;
let component:ReturnType<typeof mount>|undefined;
let cleanup:CleanupSetup;
beforeEach(() => {
  vi.clearAllMocks(); listeners.clear();
  vi.mocked(getModelStatus).mockResolvedValue({downloaded:true,loaded:true,initializing:false,error:null,path:null,name:'Parakeet v3',size_bytes:null});
  cleanup = new CleanupSetup();
  cleanup.status = {available:true,enabled:true,busy:false,loaded:true,downloaded:true,downloading:false,preparing:false,setup_error:null,unavailable_reason:null,model_name:'MiniCPM5',model_url:'',model_path:null,download_size_mb:1427,last_cleanup_status:{kind:'rejected',detail:{reason:'Protected span changed'}}};
  HTMLDialogElement.prototype.showModal = function () { this.open = true; };
  HTMLDialogElement.prototype.close = function () { this.open = false; };
});
afterEach(async () => {if(component) await unmount(component);component=undefined;cleanup.dispose();target?.remove();});
async function render(name='Parakeet v3') {target=document.createElement('div');document.body.append(target);component=mount(ModelDashboard,{target,props:{cleanup,onconfigure:vi.fn()}});flushSync();await vi.waitFor(()=>expect(target.textContent).toContain(name));flushSync();}
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
it('offers a runtime repair only for runtime failures, and runs it on click', async () => {
  vi.mocked(repairLlmRuntime).mockResolvedValue({...cleanup.status!, last_cleanup_status:{kind:'idle'}});
  await render();
  expect(target.textContent).not.toContain('Repair cleanup runtime');

  cleanup.status = {...cleanup.status!, last_cleanup_status:{kind:'unavailable',detail:{reason:'Cleanup runtime needs repair: python3 is older than 3.11'}}};
  flushSync();
  const repair = [...target.querySelectorAll('button')].find((button)=>button.textContent?.includes('Repair cleanup runtime'));
  expect(repair).toBeDefined();
  repair!.click();
  await vi.waitFor(()=>expect(repairLlmRuntime).toHaveBeenCalledOnce());
  // The store's continuation runs after the mocked promise resolves, so wait
  // for the render rather than asserting straight after the click.
  await vi.waitFor(()=>expect(target.textContent).not.toContain('Repair cleanup runtime'));
});
it('re-downloads model files that fail verification only after confirmation, with progress', async () => {
  vi.mocked(getModelStatus).mockResolvedValue({downloaded:true,loaded:true,initializing:false,error:null,path:null,name:'Parakeet v3',size_bytes:null,integrity_warning:'2 model files differ from the verified copies.'});
  let finish!:()=>void;
  vi.mocked(redownloadSpeechModel).mockReturnValueOnce(new Promise<void>((resolve)=>{finish=resolve;}));
  await render();
  await vi.waitFor(()=>expect(target.textContent).toContain('2 model files differ'));
  [...target.querySelectorAll('button')].find((button)=>button.textContent?.trim()==='Re-download speech model')!.click();flushSync();
  expect(redownloadSpeechModel).not.toHaveBeenCalled();
  const dialog = target.querySelector('dialog')!;
  expect(dialog.open).toBe(true);
  [...dialog.querySelectorAll('button')].find((button)=>button.textContent?.trim()==='Re-download')!.click();flushSync();
  expect(redownloadSpeechModel).toHaveBeenCalledOnce();
  expect(dialog.open).toBe(false);
  listeners.get('model-download-progress')!({payload:{downloaded_bytes:100_000_000,total_bytes:400_000_000}});flushSync();
  expect(target.querySelector('[role="progressbar"]')?.getAttribute('aria-valuenow')).toBe('25');
  listeners.get('asr-init-started')!();flushSync();
  expect(target.querySelector('[role="progressbar"]')).toBeNull();
  finish();
  await vi.waitFor(()=>expect(target.textContent).toContain('Model files replaced'));
});
it('stays ready and says why while an earlier speech model serves', async () => {
  vi.mocked(getModelStatus).mockResolvedValue({downloaded:true,loaded:true,initializing:false,error:null,path:null,name:'Parakeet v3',size_bytes:null,notice:'Parakeet Ultra is downloading in the background (632 MB).'});
  await render();
  const card = target.querySelector('[aria-label="Speech recognition model"]')!;
  await vi.waitFor(()=>expect(card.textContent).toContain('downloading in the background'));
  expect(card.querySelector('.model-state.ready')).not.toBeNull();
  expect(card.textContent).toContain('ready to transcribe');
});
it('offers to delete a previous speech model that is still on disk, and deletes it only after confirmation', async () => {
  const status = {downloaded:true,loaded:true,initializing:false,error:null,path:null,name:'Parakeet v3',size_bytes:null};
  const offer = () => [...target.querySelectorAll('button')].find((button)=>button.textContent?.trim()==='Delete previous model');
  await render();
  expect(offer()).toBeUndefined();

  vi.mocked(getModelStatus).mockResolvedValue({...status,previous_model:{name:'Parakeet TDT v3',size_bytes:966_514_496}});
  listeners.get('asr-init-complete')!();
  const card = target.querySelector('[aria-label="Speech recognition model"]')!;
  await vi.waitFor(()=>expect(card.textContent).toContain('Parakeet TDT v3 (967 MB), is still on this Mac'));
  offer()!.click();flushSync();
  expect(deletePreviousSpeechModel).not.toHaveBeenCalled();
  const dialog = [...target.querySelectorAll('dialog')].find((candidate)=>candidate.textContent?.includes('Delete the previous speech model?'))!;
  expect(dialog.open).toBe(true);
  expect(dialog.textContent).toContain('SottoASR deletes Parakeet TDT v3 (967 MB)');

  // The backend no longer reports the model once its folders are gone.
  vi.mocked(deletePreviousSpeechModel).mockImplementation(async () => { vi.mocked(getModelStatus).mockResolvedValue(status); return 966_514_496; });
  [...dialog.querySelectorAll('button')].find((button)=>button.textContent?.trim()==='Delete')!.click();
  await vi.waitFor(()=>expect(card.textContent).toContain('Previous model deleted. 967 MB freed.'));
  expect(deletePreviousSpeechModel).toHaveBeenCalledOnce();
  expect(dialog.open).toBe(false);
  await vi.waitFor(()=>expect(offer()).toBeUndefined());
});
it('keeps the offer and says why when the previous speech model cannot be deleted', async () => {
  vi.mocked(getModelStatus).mockResolvedValue({downloaded:true,loaded:true,initializing:false,error:null,path:null,name:'Parakeet v3',size_bytes:null,previous_model:{name:'Parakeet TDT v3',size_bytes:483_260_283}});
  vi.mocked(deletePreviousSpeechModel).mockRejectedValue('Parakeet TDT v3 is in use, so SottoASR cannot delete it.');
  await render();
  const card = target.querySelector('[aria-label="Speech recognition model"]')!;
  await vi.waitFor(()=>expect(card.textContent).toContain('Parakeet TDT v3 (483 MB)'));
  [...card.querySelectorAll('button')].find((button)=>button.textContent?.trim()==='Delete previous model')!.click();flushSync();
  [...target.querySelectorAll('dialog')].flatMap((dialog)=>[...dialog.querySelectorAll('button')]).find((button)=>button.textContent?.trim()==='Delete')!.click();
  await vi.waitFor(()=>expect(card.querySelector('[role="alert"]')?.textContent).toBe('Could not delete the previous model: Parakeet TDT v3 is in use, so SottoASR cannot delete it.'));
  expect([...card.querySelectorAll('button')].some((button)=>button.textContent?.trim()==='Delete previous model')).toBe(true);
});
it('shows the cloud engine as ready, with its notice and without local model actions', async () => {
  const notice = 'SottoASR sends each dictation to Amazon Transcribe in us-west-2 with the AWS profile work.';
  const cloud = {engine:'aws_transcribe' as const,downloaded:true,loaded:true,initializing:false,error:null,path:null,name:'AWS Transcribe',size_bytes:null,notice};
  // Leftovers of the local model must not offer local actions in cloud mode.
  vi.mocked(getModelStatus).mockResolvedValue({...cloud,integrity_warning:'2 model files differ from the verified copies.',previous_model:{name:'Parakeet TDT v3',size_bytes:966_514_496}});
  await render('AWS Transcribe');
  expect(target.querySelector('[aria-label="Model status"]')).not.toBeNull();
  const card = target.querySelector('[aria-label="Speech recognition model"]')!;
  expect(card.querySelector('.model-name')?.textContent).toBe('AWS Transcribe');
  expect(card.querySelector('.model-state.ready')?.textContent).toBe('Ready');
  expect(card.textContent).toContain('A cloud service. No speech model is in memory.');
  expect(card.textContent).toContain(notice);
  expect(card.querySelector('.integrity')).toBeNull();
  expect(card.textContent).not.toContain('Parakeet TDT v3');
  expect(card.querySelectorAll('button')).toHaveLength(0);

  vi.mocked(getModelStatus).mockResolvedValue({...cloud,error:'Could not connect to Amazon Transcribe in us-west-2. Examine your internet connection.'});
  listeners.get('asr-init-error')!();
  await vi.waitFor(()=>expect(card.querySelector('.model-state')?.textContent).toBe('Needs attention'));
  expect(card.textContent).toContain('Could not connect to Amazon Transcribe');
  expect(card.querySelectorAll('button')).toHaveLength(0);
});
it('shows AWS Bedrock as the cleanup engine, with its notice and without the local repair', async () => {
  const notice = 'SottoASR sends dictation text to Amazon Bedrock in us-west-2 with the AWS profile work. The model is Claude Haiku 5.5.';
  // The other fields describe the local model. Its problems must not show in AWS Bedrock mode.
  cleanup.status = {...cleanup.status!,engine:'aws_bedrock',notice,cloud_error:null,loaded:false,setup_error:'The cleanup runtime is broken.',last_cleanup_status:{kind:'unavailable',detail:{reason:'Choose an AWS profile for AWS Bedrock in Settings.'}}};
  await render();
  const card = target.querySelector('[aria-label="AI cleanup model"]')!;
  expect(card.querySelector('.model-name')?.textContent).toBe('AWS Bedrock');
  expect(card.querySelector('.model-state.ready')?.textContent).toBe('Ready');
  expect(card.textContent).toContain('A cloud service. No cleanup model is in memory.');
  expect(card.textContent).toContain(notice);
  expect(card.textContent).not.toContain('The cleanup runtime is broken.');
  expect(card.textContent).not.toContain('Repair cleanup runtime');

  cleanup.status = {...cleanup.status!,cloud_error:'AWS profile work was not found in ~/.aws/config or ~/.aws/credentials.'};flushSync();
  expect(card.querySelector('.model-state')?.textContent).toBe('Needs attention');
  expect(card.textContent).toContain('AWS profile work was not found in ~/.aws/config or ~/.aws/credentials.');
  expect(card.textContent).not.toContain(notice);
  expect(card.textContent).not.toContain('Repair cleanup runtime');

  cleanup.status = {...cleanup.status!,enabled:false};flushSync();
  expect(card.querySelector('.model-state')?.textContent).toBe('Off');
  expect(card.querySelector('.ready')).toBeNull();
  expect(card.textContent).not.toContain(notice);
});
