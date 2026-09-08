import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { flushSync, mount, unmount } from 'svelte';
vi.mock('@tauri-apps/api/event', () => ({listen:vi.fn(async () => vi.fn())}));
vi.mock('@tauri-apps/plugin-clipboard-manager', () => ({writeText:vi.fn()}));
vi.mock('../utils/tauri', () => ({getTranscriptions:vi.fn(), clearTranscriptions:vi.fn(), deleteTranscription:vi.fn(), exportTranscriptionsCsvFile:vi.fn()}));
import { getTranscriptions, clearTranscriptions, exportTranscriptionsCsvFile } from '../utils/tauri';
import { listen } from '@tauri-apps/api/event';
import { transcriptionStore } from '../stores/transcriptions.svelte';
import HistoryView from './history-view.svelte';
let component:ReturnType<typeof mount>|undefined;
let target:HTMLDivElement;
beforeEach(() => {
  vi.clearAllMocks(); transcriptionStore.items = []; transcriptionStore.loaded = false; transcriptionStore.error = '';
  HTMLDialogElement.prototype.showModal = function () {this.open=true;}; HTMLDialogElement.prototype.close = function () {this.open=false;};
  vi.mocked(getTranscriptions).mockResolvedValue(Array.from({length:5000},(_,i)=>({id:`history-${i}`,text:`Transcript ${i}`,created_at:'2026-09-08T00:00:00Z',duration_ms:1000,word_count:2})));
});
afterEach(async () => {if(component) await unmount(component);component=undefined;target?.remove();});
async function render() { target=document.createElement('div');document.body.append(target);component=mount(HistoryView,{target});flushSync();await vi.waitFor(()=>expect(transcriptionStore.loaded).toBe(true));flushSync(); }
const button=(label:string)=>Array.from(target.querySelectorAll('button')).find((item)=>item.textContent?.trim()===label)!;
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
it('requires confirmation before clearing and preserves entries on failure',async()=>{
  await render();button('Clear All').click();flushSync();expect(clearTranscriptions).not.toHaveBeenCalled();expect(target.querySelector('dialog')?.open).toBe(true);
  vi.mocked(clearTranscriptions).mockRejectedValueOnce(new Error('Disk full'));button('Clear all history').click();
  await vi.waitFor(()=>expect(target.textContent).toContain('Disk full'));expect(transcriptionStore.items).toHaveLength(5000);
});

it('reports export success only after native file creation succeeds',async()=>{
  await render(); vi.mocked(exportTranscriptionsCsvFile).mockRejectedValueOnce(new Error('Downloads is read-only'));
  button('Export CSV').click(); await vi.waitFor(()=>expect(target.textContent).toContain('Downloads is read-only'));
  expect(target.textContent).not.toContain('CSV saved:');
  vi.mocked(exportTranscriptionsCsvFile).mockResolvedValueOnce('/Users/test/Downloads/SottoASR-transcriptions-test.csv');
  button('Export CSV').click(); await vi.waitFor(()=>expect(target.textContent).toContain('CSV saved: /Users/test/Downloads/'));
});

it('registers live history before taking its initial authoritative snapshot',async()=>{
  let subscribed!:(off:()=>void)=>void;
  vi.mocked(listen).mockImplementationOnce(()=>new Promise((resolve)=>{subscribed=resolve;}));
  target=document.createElement('div');document.body.append(target);component=mount(HistoryView,{target});flushSync();
  expect(getTranscriptions).not.toHaveBeenCalled();
  subscribed(vi.fn());await vi.waitFor(()=>expect(getTranscriptions).toHaveBeenCalledOnce());
  await vi.waitFor(()=>expect(transcriptionStore.loaded).toBe(true));
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
