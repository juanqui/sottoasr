import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { flushSync, mount, unmount } from 'svelte';
const { listeners } = vi.hoisted(() => ({ listeners: new Map<string, (event:{payload:unknown})=>void>() }));
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(async (name:string, callback:(event:{payload:unknown})=>void) => {
  listeners.set(name, callback); return () => listeners.delete(name);
}) }));
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import type { DictationError, OverlaySnapshot } from '../utils/tauri';
import OverlayPill from './overlay-pill.svelte';
let component: ReturnType<typeof mount> | undefined;
let target: HTMLDivElement;
let latest: OverlaySnapshot;
const idle = ():OverlaySnapshot => ({revision:0,generation:0,state:'Idle',started_at_ms:null,error:null});
beforeEach(() => {
  vi.clearAllMocks(); listeners.clear(); latest=idle();
  vi.mocked(invoke).mockImplementation(async (command) => command === 'get_overlay_snapshot' ? idle() : undefined);
  vi.stubGlobal('ResizeObserver', class { observe() {} disconnect() {} });
  vi.stubGlobal('matchMedia', () => Object.assign(new EventTarget(), { matches:false }));
  const ctx={clearRect:vi.fn(),setTransform:vi.fn(),beginPath:vi.fn(),roundRect:vi.fn(),fill:vi.fn(),fillStyle:''};
  vi.spyOn(HTMLCanvasElement.prototype,'getContext').mockReturnValue(ctx as unknown as CanvasRenderingContext2D);
});
afterEach(async () => { if(component) await unmount(component); component=undefined; target?.remove(); vi.unstubAllGlobals(); });
async function render() {
  target=document.createElement('div'); document.body.append(target); component=mount(OverlayPill,{target}); flushSync();
  await vi.waitFor(() => expect(listeners.has('overlay-state')).toBe(true));
}
function publish(patch:Partial<OverlaySnapshot>) {
  latest={...latest,...patch,revision:latest.revision+1}; listeners.get('overlay-state')!({payload:latest}); flushSync();
}
function fail(event:string,payload:DictationError) { publish({error:{event,payload}}); }
const button=(label:string)=>Array.from(target.querySelectorAll('button')).find((item)=>item.textContent?.trim()===label)!;

it('recovers an error that happened before the overlay listener existed', async () => {
  vi.mocked(invoke).mockResolvedValue({revision:4,generation:1,state:'Idle',started_at_ms:null,error:{event:'transcription-error',payload:{generation:1,error:'Audio was kept.',audio_path:'/tmp/sotto_fixture.wav'}}});
  await render(); await vi.waitFor(() => expect(target.textContent).toContain('Audio was kept.'));
  button('Show audio').click(); expect(invoke).toHaveBeenCalledWith('reveal_recording_audio',{path:'/tmp/sotto_fixture.wav'});
});
it('keeps recoverable errors through Idle and waits for native dismissal acknowledgement', async () => {
  await render(); publish({generation:1,state:'Recording',started_at_ms:Date.now()}); publish({state:'Transcribing'});
  fail('transcription-error',{generation:1,error:'Decoder unavailable; audio was kept.'}); publish({state:'Idle'});
  expect(target.querySelector('.pill-container.visible')).not.toBeNull(); expect(target.textContent).toContain('Decoder unavailable');
  button('Dismiss').click(); await vi.waitFor(() => expect(target.querySelector('.error-card')).toBeNull());
  expect(invoke).toHaveBeenCalledWith('dismiss_overlay_error',{revision:latest.revision});
});
it('ignores an older snapshot response after a newer live error and Idle state', async () => {
  let resolve!:(value:OverlaySnapshot)=>void;
  vi.mocked(invoke).mockReturnValueOnce(new Promise((done)=>{resolve=done;}));
  await render(); publish({generation:1,state:'Transcribing'});
  fail('transcription-error',{generation:1,error:'New failure'}); publish({state:'Idle'});
  resolve({revision:1,generation:1,state:'Recording',started_at_ms:Date.now(),error:null});
  await Promise.resolve(); flushSync(); expect(target.textContent).toContain('New failure'); expect(button('Dismiss').disabled).toBe(false);
});
it('starts a new recording without restoring a stale predecessor error', async () => {
  await render(); publish({generation:1}); fail('transcription-error',{generation:1,error:'Old failure'});
  const old=latest; publish({generation:2,state:'Recording',started_at_ms:Date.now(),error:null});
  listeners.get('overlay-state')!({payload:old}); flushSync(); expect(target.querySelector('.error-card')).toBeNull();
});
it('never claims text was copied when fallback failed', async () => {
  await render(); publish({generation:3}); fail('paste-error',{generation:3,error:'Clipboard unavailable',clipboard_available:false});
  expect(target.textContent).not.toContain('Text is on the clipboard'); button('History').click(); expect(invoke).toHaveBeenCalledWith('open_transcription_history',undefined);
  fail('paste-error',{generation:3,error:'Permission needed',clipboard_available:true}); expect(target.textContent).toContain('Text is on the clipboard');
});
it('does not dismiss a newer failure when an old dismissal completes late', async () => {
  let finish!:()=>void;
  await render(); vi.mocked(invoke).mockImplementationOnce(()=>new Promise<void>((resolve)=>{finish=resolve;}));
  publish({generation:1}); fail('transcription-error',{generation:1,error:'First failure'}); button('Dismiss').click();
  publish({generation:2,state:'Recording',started_at_ms:Date.now(),error:null});
  fail('transcription-error',{generation:2,error:'Second failure'}); publish({state:'Idle'});
  finish(); await vi.waitFor(()=>expect(button('Dismiss').disabled).toBe(false)); expect(target.textContent).toContain('Second failure');
});
it('focuses only recovery, supports Escape, and removes its keyboard listener', async () => {
  await render(); publish({generation:1,state:'Recording',started_at_ms:Date.now()}); expect(document.activeElement).toBe(document.body);
  fail('transcription-error',{generation:1,error:'Recoverable failure',audio_path:'/tmp/sotto_fixture.wav'}); publish({state:'Idle'});
  await vi.waitFor(()=>expect(document.activeElement).toBe(button('Show audio')));
  window.dispatchEvent(new KeyboardEvent('keydown',{key:'Escape',cancelable:true})); await vi.waitFor(()=>expect(target.querySelector('.error-card')).toBeNull());
  await unmount(component!); component=undefined; vi.mocked(invoke).mockClear();
  window.dispatchEvent(new KeyboardEvent('keydown',{key:'Escape'})); expect(invoke).not.toHaveBeenCalled();
});
it('retains failed history-save recovery and prevents focus or dismissal while pasting', async () => {
  await render(); publish({generation:4,state:'Pasting'}); fail('history-save-error',{generation:4,error:'History could not be saved.',audio_path:'/tmp/sotto_fixture.wav'});
  expect(button('Dismiss').disabled).toBe(true); window.dispatchEvent(new KeyboardEvent('keydown',{key:'Escape'})); expect(vi.mocked(invoke).mock.calls.some(([command])=>command==='dismiss_overlay_error')).toBe(false);
  publish({state:'Idle'}); expect(target.textContent).toContain('History not saved'); button('Show audio').click(); expect(invoke).toHaveBeenCalledWith('reveal_recording_audio',{path:'/tmp/sotto_fixture.wav'});
});
it('waits for listener registration before requesting the snapshot and skips it after disposal', async () => {
  let registered!:(unlisten:()=>void)=>void;
  vi.mocked(listen).mockImplementationOnce(async (_name,handler) => {
    listeners.set('overlay-state',handler as (event:{payload:unknown})=>void);
    return new Promise((resolve)=>{registered=resolve;});
  });
  await render(); expect(invoke).not.toHaveBeenCalled(); await unmount(component!); component=undefined;
  const off=vi.fn(); registered(off); await vi.waitFor(()=>expect(off).toHaveBeenCalledOnce()); expect(invoke).not.toHaveBeenCalled();
});
it('labels experimental output as a History suggestion instead of cleaned dictation', async () => {
  await render(); publish({generation:1,state:'CleaningUp'});
  expect(target.textContent).toContain('Preparing suggestion');
  listeners.get('llm-cleanup-status')!({payload:{kind:'suggested',detail:{elapsed_ms:20}}}); flushSync();
  expect(target.textContent).toContain('Suggestion in History');
  expect(target.textContent).not.toContain('Cleaned');
});
