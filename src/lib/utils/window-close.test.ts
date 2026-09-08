import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type { Event, EventCallback } from '@tauri-apps/api/event';
import defaultCapability from '../../../src-tauri/capabilities/default.json?raw';
import settingsCloseCapability from '../../../src-tauri/capabilities/settings-close.json?raw';

import { getCurrentWindow } from '@tauri-apps/api/window';

const capabilities = [defaultCapability, settingsCloseCapability].map((text) =>
  JSON.parse(text) as { windows: string[]; permissions: string[] });
let callback: EventCallback<unknown>;
const invoke = vi.fn<(command: string, args?: unknown) => Promise<void>>();

beforeEach(() => {
  vi.clearAllMocks();
  vi.stubGlobal('__TAURI_INTERNALS__', { metadata: { currentWindow: { label: 'settings' } }, invoke });
  invoke.mockImplementation(async (command, args) => {
    if (command !== 'plugin:window|destroy') throw new Error(`Unexpected IPC ${command}`);
    const label = (args as { label: string }).label;
    const permitted = capabilities.some((capability) => capability.windows.includes(label)
      && capability.permissions.includes('core:window:allow-destroy'));
    if (!permitted) throw new Error('window.destroy not allowed by configured capabilities');
  });
});
afterEach(() => vi.unstubAllGlobals());

async function requestClose(prevent: boolean) {
  // Keep the actual installed SDK's close-request wrapper. Replacing the entire
  // Window API would miss its implicit destroy() call and required permission.
  const window = getCurrentWindow();
  vi.spyOn(window, 'listen').mockImplementation(async (_event, handler) => {
    callback = handler as EventCallback<unknown>;
    return () => {};
  });
  await window.onCloseRequested((event) => { if (prevent) event.preventDefault(); });
  await callback({ event: 'tauri://close-requested', id: 1, payload: null } satisfies Event<unknown>);
}

it('permits the real Settings X handler to destroy the window after an accepted close', async () => {
  await requestClose(false);
  expect(invoke).toHaveBeenCalledWith('plugin:window|destroy', { label: 'settings' }, undefined);
  const grant = capabilities.find((capability) => capability.permissions.includes('core:window:allow-destroy'));
  expect(grant?.windows).toEqual(['settings']);
});

it('keeps the real close handler from destroying a window when unsaved changes block closing', async () => {
  await requestClose(true);
  expect(invoke).not.toHaveBeenCalled();
});
