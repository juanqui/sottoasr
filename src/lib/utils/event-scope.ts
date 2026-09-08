import { listen } from '@tauri-apps/api/event';
import type { EventCallback } from '@tauri-apps/api/event';

/** Own asynchronous event registrations, including those completed after disposal. */
export function createEventScope(onError: (error: unknown) => void = console.error) {
  let disposed = false;
  const cleanups: Array<() => void> = [];
  return {
    listen<T>(event: string, handler: EventCallback<T>) {
      return listen<T>(event, (value) => {
        if (disposed) return;
        try { void Promise.resolve(handler(value)).catch((error) => { if (!disposed) onError(error); }); }
        catch (error) { onError(error); }
      })
        .then((unlisten) => { if (disposed) unlisten(); else cleanups.push(unlisten); })
        .catch((error) => { if (!disposed) onError(error); });
    },
    add(cleanup: () => void) { if (disposed) cleanup(); else cleanups.push(cleanup); },
    dispose() {
      disposed = true;
      cleanups.splice(0).forEach((cleanup) => cleanup());
    },
  };
}
