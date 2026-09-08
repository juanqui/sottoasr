import { expect, it, vi } from 'vitest';
vi.mock('@tauri-apps/api/event', () => ({ listen:vi.fn() }));
import { listen } from '@tauri-apps/api/event';
import { createEventScope } from './event-scope';
it('unregisters a listener that resolves after disposal and ignores its callbacks', async () => {
  let resolve!: (fn:()=>void)=>void;
  vi.mocked(listen).mockReturnValueOnce(new Promise((yes) => { resolve = yes; }));
  const handler = vi.fn(); const cleanup = vi.fn(); const scope = createEventScope();
  scope.listen('test', handler); scope.dispose();
  vi.mocked(listen).mock.calls.at(-1)![1]({event:'test', id:1, payload:'late'});
  resolve(cleanup); await Promise.resolve();
  expect(cleanup).toHaveBeenCalledOnce(); expect(handler).not.toHaveBeenCalled();
});
