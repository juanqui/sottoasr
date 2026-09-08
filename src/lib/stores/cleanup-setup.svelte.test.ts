import { beforeEach, expect, it, vi } from 'vitest';
vi.mock('../utils/tauri', () => ({ getLlmStatus:vi.fn(), prepareLlmModel:vi.fn() }));
import { getLlmStatus, prepareLlmModel } from '../utils/tauri';
import { CleanupSetup } from './cleanup-setup.svelte';
import type { LlmStatus } from '../utils/tauri';
const ready: LlmStatus = { available:true, unavailable_reason:null, downloaded:true, downloading:false, loaded:true, preparing:false, setup_error:null, model_name:'Test', model_url:'https://example.com', model_path:null, download_size_mb:227, update_available:false, last_cleanup_status:{kind:'idle'} };
const deferred = <T>() => { let resolve!: (value:T) => void; const promise = new Promise<T>((yes) => { resolve = yes; }); return {promise,resolve}; };
beforeEach(() => vi.resetAllMocks());
it('shares rapid enable clicks and activates only after successful preparation', async () => {
  const work = deferred<LlmStatus>(); vi.mocked(prepareLlmModel).mockReturnValue(work.promise);
  const setup = new CleanupSetup(); const enabled = vi.fn();
  const pending = setup.enable(enabled); await setup.enable(enabled);
  expect(prepareLlmModel).toHaveBeenCalledTimes(1); expect(enabled).not.toHaveBeenCalled();
  work.resolve(ready); await pending;
  expect(enabled).toHaveBeenCalledOnce(); expect(setup.pending).toBe(false);
});
it.each(['cancel','dispose'] as const)('ignores preparation after %s', async (action) => {
  const work = deferred<LlmStatus>(); vi.mocked(prepareLlmModel).mockReturnValue(work.promise);
  const setup = new CleanupSetup(); const enabled = vi.fn(); const pending = setup.enable(enabled);
  setup[action](); work.resolve(ready); await pending;
  expect(enabled).not.toHaveBeenCalled(); expect(setup.pending).toBe(false);
});
it('keeps cleanup off on download/runtime failure and offers a fresh retry', async () => {
  vi.mocked(prepareLlmModel).mockRejectedValueOnce(new Error('Network unavailable')).mockResolvedValueOnce(ready);
  const setup = new CleanupSetup(); const enabled = vi.fn();
  await setup.enable(enabled); expect(setup.error).toBe('Network unavailable'); expect(enabled).not.toHaveBeenCalled();
  await setup.enable(enabled); expect(setup.error).toBe(''); expect(enabled).toHaveBeenCalledOnce();
});
it('does not treat downloaded but unloaded model as successful setup', async () => {
  vi.mocked(prepareLlmModel).mockResolvedValue({ ...ready, loaded:false });
  const setup = new CleanupSetup(); const enabled = vi.fn(); await setup.enable(enabled);
  expect(enabled).not.toHaveBeenCalled(); expect(setup.error).toContain('not ready');
});
it('prevents stale status replies replacing a completed setup', async () => {
  const status = deferred<LlmStatus>(); vi.mocked(getLlmStatus).mockReturnValue(status.promise); vi.mocked(prepareLlmModel).mockResolvedValue(ready);
  const setup = new CleanupSetup(); const refresh = setup.refresh(); await setup.enable(vi.fn());
  status.resolve({...ready, downloaded:false, loaded:false}); await refresh;
  expect(setup.status?.loaded).toBe(true); expect(setup.loading).toBe(false);
});
it('clears the Save-to-enable reminder when a ready activation is cancelled', async () => {
  vi.mocked(prepareLlmModel).mockResolvedValue(ready);
  const setup = new CleanupSetup(); await setup.enable(vi.fn());
  expect(setup.notice).toContain('Ready. Save'); expect(setup.pending).toBe(false);
  setup.cancel(); expect(setup.notice).toBe('');
});

it('does not let a poll started during preparation erase ready acknowledgement', async () => {
  const preparing=deferred<LlmStatus>(); const reading=deferred<LlmStatus>();
  vi.mocked(prepareLlmModel).mockReturnValue(preparing.promise); vi.mocked(getLlmStatus).mockReturnValue(reading.promise);
  const setup=new CleanupSetup(); const activation=setup.enable(vi.fn()); const poll=setup.refresh();
  preparing.resolve(ready); await activation;
  reading.resolve({...ready,loaded:false,preparing:true}); await poll;
  expect(setup.status?.loaded).toBe(true); expect(setup.status?.preparing).toBe(false);
});
it('coalesces status reads and clears stale loaded status after a read failure', async () => {
  const reading=deferred<LlmStatus>();vi.mocked(getLlmStatus).mockReturnValueOnce(reading.promise);
  const setup=new CleanupSetup(); const first=setup.refresh();await setup.refresh();
  expect(getLlmStatus).toHaveBeenCalledOnce();reading.resolve(ready);await first;
  vi.mocked(getLlmStatus).mockRejectedValueOnce(new Error('Connection lost'));await setup.refresh();
  expect(setup.status).toBeNull();expect(setup.error).toContain('Connection lost');
});
