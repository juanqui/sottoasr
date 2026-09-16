import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { flushSync, mount, unmount } from 'svelte';
vi.mock('@tauri-apps/api/event', () => ({listen:vi.fn(async () => vi.fn())}));
vi.mock('../utils/tauri', () => ({getRecoverableRecordings:vi.fn(), getRecoveryNotice:vi.fn(), recoverRecording:vi.fn(), revealRecoveryRecording:vi.fn()}));
import { getRecoverableRecordings, getRecoveryNotice, recoverRecording, revealRecoveryRecording } from '../utils/tauri';
import { listen } from '@tauri-apps/api/event';
import { transcriptionStore } from '../stores/transcriptions.svelte';
import RecordingRecovery from './recording-recovery.svelte';

const recovered = {id:'recovered-1',text:'Recovered narration',created_at:'2026-09-15T10:00:00Z',duration_ms:926520,word_count:3};
const pendingItem = {id:'sotto-uuid-1',audio_path:'/Users/test/Library/Application Support/com.sottoasr.app/recordings/sotto_6bdc0efe-f53b-48f1-9bdf-a1ea6a8fda4d.wav',created_at:'2026-09-15T10:00:00Z',duration_ms:926520,size_bytes:44468562,error:null};

let component:ReturnType<typeof mount>|undefined;
let target:HTMLDivElement;
beforeEach(() => {
  vi.clearAllMocks(); transcriptionStore.items = []; transcriptionStore.loaded = false; transcriptionStore.error = '';
  vi.mocked(getRecoverableRecordings).mockResolvedValue([]);
  vi.mocked(getRecoveryNotice).mockResolvedValue(null);
  vi.mocked(recoverRecording).mockResolvedValue(recovered as never);
  vi.mocked(revealRecoveryRecording).mockResolvedValue(undefined);
});
afterEach(async () => {if(component) await unmount(component);component=undefined;target?.remove();});
function render() { target=document.createElement('div');document.body.append(target);component=mount(RecordingRecovery,{target});flushSync(); }
const button=(label:string)=>Array.from(target.querySelectorAll('button')).find((item)=>item.textContent?.trim()===label)!;

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => { resolve = done; });
  return { promise, resolve };
}

it('shows pending audio with the exact path, created time, and duration', async () => {
  vi.mocked(getRecoverableRecordings).mockResolvedValue([pendingItem]);
  render();
  await vi.waitFor(()=>expect(target.querySelector('.recovery-path')?.textContent).toBe(pendingItem.audio_path));
  expect(target.textContent).toContain('15:26'); // 926520 ms
});

it('hides the section after a clean exit with no pending audio and no notice', async () => {
  render();
  await vi.waitFor(()=>expect(getRecoverableRecordings).toHaveBeenCalled());
  await vi.waitFor(()=>expect(getRecoveryNotice).toHaveBeenCalled());
  await vi.waitFor(()=>expect(target.querySelector('.recovery')).toBeNull());
});

it('shows the unclean-exit notice even when no recordings are found', async () => {
  vi.mocked(getRecoveryNotice).mockResolvedValue('SottoASR did not shut down cleanly. No pending recording was found.');
  render();
  await vi.waitFor(()=>expect(target.querySelector('.recovery-notice')?.textContent).toContain('did not shut down cleanly'));
  expect(target.querySelector('.recovery-item')).toBeNull();
});

it('shows a notice read failure while the list still works', async () => {
  vi.mocked(getRecoverableRecordings).mockResolvedValueOnce([pendingItem]);
  vi.mocked(getRecoveryNotice).mockRejectedValueOnce(new Error('Marker unreadable'));
  render();
  await vi.waitFor(()=>expect(target.querySelector('.recovery-error')?.textContent).toContain('Marker unreadable'));
  expect(target.querySelector('.recovery-item')).not.toBeNull(); // the list side is unaffected
  button('Retry').click(); // retries only the notice read
  await vi.waitFor(()=>expect(getRecoverableRecordings).toHaveBeenCalledOnce());
  await vi.waitFor(()=>expect(target.querySelector('.recovery-error')).toBeNull());
});

it('keeps the section visible when the list check fails and recovers on retry', async () => {
  vi.mocked(getRecoverableRecordings).mockRejectedValueOnce(new Error('Directory unreadable'));
  render();
  await vi.waitFor(()=>expect(target.querySelector('.recovery-error')?.textContent).toContain('Directory unreadable'));
  button('Retry').click();
  await vi.waitFor(()=>expect(target.querySelector('.recovery-error')).toBeNull());
  expect(getRecoverableRecordings).toHaveBeenCalledTimes(2);
});

it('reprocesses once: disables the action while active, saves to history, and refreshes pending items', async () => {
  vi.mocked(getRecoverableRecordings).mockResolvedValueOnce([pendingItem]);
  const {promise, resolve} = deferred<typeof recovered>();
  vi.mocked(recoverRecording).mockReturnValueOnce(promise as never);
  render();
  await vi.waitFor(()=>expect(target.querySelector('.recovery-item')).not.toBeNull());
  button('Reprocess').click();
  flushSync();
  expect(target.querySelector<HTMLButtonElement>('.reprocess')!.disabled).toBe(true);
  expect(target.querySelector<HTMLButtonElement>('.reveal')!.disabled).toBe(false);
  resolve(recovered);
  await vi.waitFor(()=>expect(transcriptionStore.items.map((item)=>item.id)).toContain('recovered-1'));
  await vi.waitFor(()=>expect(target.querySelector('.recovery-item')).toBeNull());
  expect(target.querySelector('.recovery-status')).not.toBeNull();
});

it('keeps the pending item and offers retry when reprocessing fails, then succeeds on retry', async () => {
  vi.mocked(getRecoverableRecordings).mockResolvedValueOnce([pendingItem]);
  vi.mocked(recoverRecording).mockRejectedValueOnce(new Error('Model failed to load'));
  render();
  await vi.waitFor(()=>expect(target.querySelector('.recovery-item')).not.toBeNull());
  button('Reprocess').click();
  await vi.waitFor(()=>expect(target.textContent).toContain('Model failed to load'));
  expect(target.querySelector('.recovery-item')).not.toBeNull(); // audio and row survive failure
  expect(transcriptionStore.items).toHaveLength(0);
  expect(target.querySelector<HTMLButtonElement>('.reprocess')!.disabled).toBe(false);
  button('Reprocess').click(); // retry with the success default
  await vi.waitFor(()=>expect(target.querySelector('.recovery-item')).toBeNull());
  expect(transcriptionStore.items.map((item)=>item.id)).toContain('recovered-1');
});

it('a late stale list response cannot resurrect a recording reprocessing completed', async () => {
  const first = deferred<typeof pendingItem[]>();
  const second = deferred<typeof pendingItem[]>();
  vi.mocked(getRecoverableRecordings).mockReturnValueOnce(first.promise as never).mockReturnValueOnce(second.promise as never);
  render();
  await vi.waitFor(()=>expect(getRecoverableRecordings).toHaveBeenCalledOnce());
  // A change event starts a second read while the first read is still in flight.
  const [, handler] = vi.mocked(listen).mock.calls.find(([event])=>event==='recovery-recordings-changed')!;
  await handler({event:'recovery-recordings-changed', id:1, payload:undefined});
  await vi.waitFor(()=>expect(getRecoverableRecordings).toHaveBeenCalledTimes(2));
  second.resolve([pendingItem]); // the newer snapshot lands first
  await vi.waitFor(()=>expect(target.querySelector('.recovery-item')).not.toBeNull());
  first.resolve([pendingItem]);  // the older snapshot arrives late and must be ignored
  vi.mocked(getRecoverableRecordings).mockResolvedValueOnce([]);
  button('Reprocess').click();   // completes against the newer snapshot's item
  await vi.waitFor(()=>expect(target.querySelector('.recovery-item')).toBeNull());
  await Promise.resolve(); flushSync();
  expect(target.querySelector('.recovery-item')).toBeNull(); // the ignored stale response never owned the list
});

it('registers its event listener before the initial read so a startup event cannot be missed', async () => {
  let subscribed!:(off:()=>void)=>void;
  vi.mocked(listen).mockImplementationOnce(()=>new Promise((resolve)=>{subscribed=resolve;}));
  render();
  expect(getRecoverableRecordings).not.toHaveBeenCalled();
  subscribed(vi.fn());
  await vi.waitFor(()=>expect(getRecoverableRecordings).toHaveBeenCalled());
});

it('removes its listener on unmount so a late event cannot refresh a dead view', async () => {
  const unlisten = vi.fn();
  vi.mocked(listen).mockResolvedValueOnce(unlisten);
  render();
  await vi.waitFor(()=>expect(listen).toHaveBeenCalled());
  const [, handler] = vi.mocked(listen).mock.calls.find(([event])=>event==='recovery-recordings-changed')!;
  await unmount(component!); component=undefined;
  expect(unlisten).toHaveBeenCalled();
  const reads = vi.mocked(getRecoverableRecordings).mock.calls.length;
  await handler({event:'recovery-recordings-changed', id:1, payload:undefined});
  expect(vi.mocked(getRecoverableRecordings).mock.calls.length).toBe(reads);
});

it('reports Show in Finder failures per item without blocking reprocess', async () => {
  vi.mocked(getRecoverableRecordings).mockResolvedValueOnce([pendingItem]);
  vi.mocked(revealRecoveryRecording).mockRejectedValueOnce(new Error('Finder unavailable'));
  render();
  await vi.waitFor(()=>expect(target.querySelector('.recovery-item')).not.toBeNull());
  button('Show in Finder').click();
  await vi.waitFor(()=>expect(target.textContent).toContain('Finder unavailable'));
  expect(revealRecoveryRecording).toHaveBeenCalledWith(pendingItem.id); // commands take the recovery id, not a path
  expect(target.querySelector<HTMLButtonElement>('.reprocess')!.disabled).toBe(false);
});

it('disables Reprocess but keeps Show in Finder for an item the backend flagged as unimportable', async () => {
  const broken = {...pendingItem, error:'The audio was kept, but it cannot be opened for reprocessing.'};
  vi.mocked(getRecoverableRecordings).mockResolvedValueOnce([broken]);
  render();
  await vi.waitFor(()=>expect(target.querySelector('.recovery-item')).not.toBeNull());
  expect(target.querySelector('.recovery-item-error')?.textContent).toContain('cannot be opened');
  expect(target.querySelector<HTMLButtonElement>('.reprocess')!.disabled).toBe(true);
  expect(target.querySelector<HTMLButtonElement>('.reveal')!.disabled).toBe(false);
  button('Show in Finder').click();
  await vi.waitFor(()=>expect(revealRecoveryRecording).toHaveBeenCalledWith(broken.id));
});
