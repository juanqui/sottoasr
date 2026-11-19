import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { flushSync, mount, unmount } from 'svelte';
vi.mock('@tauri-apps/api/event', () => ({listen:vi.fn(async () => vi.fn())}));
vi.mock('@tauri-apps/plugin-clipboard-manager', () => ({writeText:vi.fn()}));
vi.mock('../utils/tauri', () => ({getTranscriptions:vi.fn(), clearTranscriptions:vi.fn(), deleteTranscription:vi.fn(), exportTranscriptionsCsvFile:vi.fn(), revealCsvExport:vi.fn(), getRecoverableRecordings:vi.fn(async()=>[]), getRecoveryNotice:vi.fn(async()=>null), recoverRecording:vi.fn(), revealRecoveryRecording:vi.fn(), deleteRecoveryRecording:vi.fn()}));
import { getTranscriptions, clearTranscriptions, deleteTranscription, exportTranscriptionsCsvFile, revealCsvExport, getRecoverableRecordings } from '../utils/tauri';
import { listen } from '@tauri-apps/api/event';
import { transcriptionStore } from '../stores/transcriptions.svelte';
import HistoryView from './history-view.svelte';
let component:ReturnType<typeof mount>|undefined;
let target:HTMLDivElement;
beforeEach(() => {
  vi.clearAllMocks(); transcriptionStore.items = []; transcriptionStore.loaded = false; transcriptionStore.error = '';
  // clearAllMocks keeps implementations set with mockImplementation; pin the default each test.
  vi.mocked(listen).mockImplementation(async () => vi.fn<() => void>());
  vi.mocked(getRecoverableRecordings).mockResolvedValue([]);
  HTMLDialogElement.prototype.showModal = function () {this.open=true;}; HTMLDialogElement.prototype.close = function () {this.open=false;};
  vi.mocked(getTranscriptions).mockResolvedValue(Array.from({length:5000},(_,i)=>({id:`history-${i}`,text:`Transcript ${i}`,created_at:'2026-09-08T00:00:00Z',duration_ms:1000,word_count:2})));
});
afterEach(async () => {if(component) await unmount(component);component=undefined;target?.remove();});
async function render() { target=document.createElement('div');document.body.append(target);component=mount(HistoryView,{target});flushSync();await vi.waitFor(()=>expect(transcriptionStore.loaded).toBe(true));flushSync(); }
const button=(label:string)=>Array.from(target.querySelectorAll('button')).find((item)=>item.textContent?.trim()===label)!;
// The recovery section has its own dialog; find the one that is showing.
const openDialog=()=>Array.from(target.querySelectorAll('dialog')).find((element)=>element.open);
it('bounds mounted rows while keeping older entries searchable and paged',async()=>{
  await render();expect(target.querySelectorAll('.history-item')).toHaveLength(50);expect(target.textContent).toContain('1–50 of 5000');
  target.querySelector<HTMLButtonElement>('.item-body')!.click();flushSync();
  target.querySelector('.history-list')!.scrollTop=400;
  button('Next').click();flushSync();expect(target.textContent).toContain('51–100 of 5000');expect(target.querySelector('.history-list')!.scrollTop).toBe(0);
  button('Previous').click();flushSync();expect(target.querySelector('.item-body')!.getAttribute('aria-expanded')).toBe('true');
  const input=target.querySelector<HTMLInputElement>('.search-input')!;input.value='Transcript 4999';input.dispatchEvent(new Event('input',{bubbles:true}));flushSync();
  expect(target.querySelectorAll('.history-item')).toHaveLength(1);expect(target.textContent).toContain('Transcript 4999');
  expect(transcriptionStore.items).toHaveLength(5000);
});
it('requires confirmation before clearing, says audio goes too, and preserves entries on failure',async()=>{
  await render();button('Clear All').click();flushSync();expect(clearTranscriptions).not.toHaveBeenCalled();expect(openDialog()).toBeDefined();
  expect(openDialog()!.querySelector('p')?.textContent).toContain('audio kept with it');
  vi.mocked(clearTranscriptions).mockRejectedValueOnce(new Error('Disk full'));button('Clear all history').click();
  await vi.waitFor(()=>expect(target.textContent).toContain('Disk full'));expect(transcriptionStore.items).toHaveLength(5000);
});

it('names the retained audio before deleting one entry and labels the row button with its entry',async()=>{
  await render();const before=transcriptionStore.items.length;const firstId=transcriptionStore.items[0].id;
  const deletes=Array.from(target.querySelectorAll<HTMLButtonElement>('.action-btn.delete'));
  expect(deletes[0].getAttribute('aria-label')).toMatch(/^Delete transcription from /);
  deletes[0].click();flushSync();
  expect(openDialog()!.querySelector('p')?.textContent).toContain('deletes the audio kept with it');
  expect(deleteTranscription).not.toHaveBeenCalled();
  openDialog()!.querySelector<HTMLButtonElement>('button.confirm')!.click();
  await vi.waitFor(()=>expect(transcriptionStore.items).toHaveLength(before-1));
  expect(deleteTranscription).toHaveBeenCalledWith(firstId);
});

it('reports export success only after native file creation succeeds',async()=>{
  await render(); vi.mocked(exportTranscriptionsCsvFile).mockRejectedValueOnce(new Error('Downloads is read-only'));
  button('Export CSV').click(); await vi.waitFor(()=>expect(target.textContent).toContain('Downloads is read-only'));
  expect(target.textContent).not.toContain('CSV saved:');
  vi.mocked(exportTranscriptionsCsvFile).mockResolvedValueOnce('/Users/test/Downloads/SottoASR-transcriptions-test.csv');
  button('Export CSV').click(); await vi.waitFor(()=>expect(target.textContent).toContain('CSV saved: /Users/test/Downloads/'));
  button('Show in Finder').click(); expect(revealCsvExport).toHaveBeenCalledWith('/Users/test/Downloads/SottoASR-transcriptions-test.csv');
});

it('registers live history before taking its initial authoritative snapshot',async()=>{
  let subscribed!:(off:()=>void)=>void;
  vi.mocked(listen).mockImplementation((event:string)=>event==='transcription-complete'?new Promise((resolve)=>{subscribed=resolve;}):Promise.resolve(vi.fn()));
  target=document.createElement('div');document.body.append(target);component=mount(HistoryView,{target});flushSync();
  expect(getTranscriptions).not.toHaveBeenCalled();
  subscribed(vi.fn());await vi.waitFor(()=>expect(getTranscriptions).toHaveBeenCalledOnce());
  await vi.waitFor(()=>expect(transcriptionStore.loaded).toBe(true));
});

it('drops entries that retention removed while the window is open',async()=>{
  await render();const before=transcriptionStore.items.length;
  const [, handler]=vi.mocked(listen).mock.calls.find(([event])=>event==='history-changed')!;
  await handler({event:'history-changed',id:1,payload:{removed_ids:['history-10','history-11']}});flushSync();
  expect(transcriptionStore.items).toHaveLength(before-2);expect(target.textContent).toContain(`1–50 of ${before-2}`);
  expect(transcriptionStore.items.some((item)=>item.id==='history-10')).toBe(false);
});

it('filters cleanup issues, keeps diagnostics readable, and clears search/filter together', async () => {
  vi.mocked(getTranscriptions).mockResolvedValue([
    {id:'good',text:'Cleaned narration',created_at:'2026-09-08T12:00:00Z',duration_ms:1000,word_count:2,llm_applied:true,llm_cleanup_status:{kind:'applied',detail:{elapsed_ms:100}}},
    {id:'rejected',text:'Original narration',created_at:'2026-09-07T12:00:00Z',duration_ms:1000,word_count:2,llm_cleanup_status:{kind:'rejected',detail:{reason:'Protected span changed'}}},
  ]);
  await render();button('Cleanup issues').click();flushSync();
  expect(target.querySelectorAll('.history-item')).toHaveLength(1);
  expect(target.querySelector('.date-group')).toBeTruthy();
  target.querySelector<HTMLButtonElement>('.item-body')!.click();flushSync();
  expect(target.querySelector('.cleanup-explanation')?.textContent).toContain('The model responded');
  expect(target.querySelector('.cleanup-explanation')?.textContent).toContain('Protected span changed');
  button('Clear filters').click();flushSync();expect(target.querySelectorAll('.history-item')).toHaveLength(2);
});
