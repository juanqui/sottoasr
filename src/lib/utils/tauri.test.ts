import { beforeAll, describe, expect, it, vi } from 'vitest';
import libSource from '../../../src-tauri/src/lib.rs?raw';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => undefined) }));

import { invoke } from '@tauri-apps/api/core';
import * as api from './tauri';

// Registered in lib.rs but not called from the frontend; they stay as they are.
const REGISTERED_WITHOUT_FRONTEND_CALLER = ['apply_shortcuts', 'fix_accessibility_permission', 'open_input_monitoring_settings'];

// The permissions that allow each plugin function the frontend may import.
// Which windows need them is worked out from the code (see `filesByWindow`).
const PLUGIN_PERMISSIONS: Record<string, string[]> = {
  '@tauri-apps/plugin-clipboard-manager:writeText': ['clipboard-manager:allow-write-text'],
  '@tauri-apps/plugin-log:error': ['log:default', 'log:allow-log'],
  '@tauri-apps/plugin-log:warn': ['log:default', 'log:allow-log'],
};

const WRAPPER_FILE = '/src/lib/utils/tauri.ts';
const sources = import.meta.glob<string>(['/src/**/*.ts', '/src/**/*.svelte', '!/src/**/*.test.ts'], { query: '?raw', import: 'default', eager: true });

interface Capability { identifier: string; windows?: string[]; permissions: Array<string | { identifier: string }> }
const capabilities = Object.values(import.meta.glob<string>('/src-tauri/capabilities/*.json', { query: '?raw', import: 'default', eager: true }))
  .map((text) => JSON.parse(text) as Capability);

/** Command names in lib.rs's `generate_handler![...]`, read at run time so other changes to the list are seen. */
function registeredCommands(source: string): string[] {
  const marker = 'generate_handler![';
  const start = source.indexOf(marker);
  if (start === -1) throw new Error('lib.rs has no generate_handler! list');
  if (source.indexOf(marker, start + 1) !== -1) throw new Error('lib.rs has more than one generate_handler! list');
  let depth = 1;
  let body = '';
  for (let index = start + marker.length; index < source.length; index++) {
    if (source.startsWith('//', index) || source.startsWith('/*', index)) {
      const end = source.indexOf(source[index + 1] === '/' ? '\n' : '*/', index);
      if (end === -1) break;
      index = source[index + 1] === '/' ? end : end + 1;
      continue;
    }
    const char = source[index];
    if (char === '[') depth++;
    if (char === ']' && --depth === 0) {
      return body.replace(/#\[[^\]]*\]/g, '').split(',').map((entry) => entry.trim()).filter(Boolean)
        .map((path) => path.split('::').at(-1)!);
    }
    body += char;
  }
  throw new Error('generate_handler! list is not closed');
}

/** Command name each exported wrapper invokes, found by calling it against the mocked `invoke`. */
async function wrapperCommands(): Promise<Map<string, string>> {
  const commands = new Map<string, string>();
  for (const [name, value] of Object.entries(api)) {
    if (typeof value !== 'function') continue;
    vi.mocked(invoke).mockClear();
    await (value as unknown as () => Promise<unknown>)();
    const calls = vi.mocked(invoke).mock.calls;
    if (calls.length !== 1) throw new Error(`${name} must invoke exactly one command, invoked ${calls.length}`);
    commands.set(String(calls[0][0]), name);
  }
  return commands;
}

/** Plugin functions one source file imports, as `module:name`. */
function pluginImports(source: string): string[] {
  const found: string[] = [];
  for (const [, clause, module] of source.matchAll(/import\s+([^;'"]*?)\s+from\s*['"](@tauri-apps\/plugin-[\w-]+)['"]/g)) {
    if (clause.startsWith('type ')) continue;
    const names = /^\{([^}]*)\}$/.exec(clause.trim());
    if (!names) { found.push(`${module}:(use named imports)`); continue; }
    for (const specifier of names[1].split(',').map((part) => part.trim()).filter(Boolean)) {
      if (!specifier.startsWith('type ')) found.push(`${module}:${specifier.split(/\s+as\s+/)[0]}`);
    }
  }
  return found;
}

/** The source file a relative import names; undefined for styles and assets. */
function resolveImport(importer: string, specifier: string): string | undefined {
  const parts = importer.split('/').slice(0, -1);
  for (const part of specifier.split('/')) {
    if (part === '..') parts.pop();
    else if (part !== '.') parts.push(part);
  }
  const path = parts.join('/');
  return [path, `${path}.ts`].find((candidate) => candidate in sources);
}

/**
 * The source files each window loads: its entry script `src/<window>.ts` and
 * everything reachable from it through relative imports. Type-only imports
 * are skipped because they load nothing.
 */
function filesByWindow(): Map<string, Set<string>> {
  const windows = new Map<string, Set<string>>();
  for (const entry of Object.keys(sources).filter((path) => /^\/src\/[^/]+\.ts$/.test(path))) {
    const reached = new Set<string>();
    const pending = [entry];
    while (pending.length) {
      const file = pending.pop()!;
      if (reached.has(file)) continue;
      reached.add(file);
      for (const [, clause, specifier] of sources[file].matchAll(/(?:import|export)\s+(?:([^;'"]*?)\s+from\s*)?['"](\.{1,2}\/[^'"]+)['"]/g)) {
        if (clause?.startsWith('type ')) continue;
        const target = resolveImport(file, specifier);
        if (target) pending.push(target);
      }
    }
    windows.set(entry.slice('/src/'.length, -'.ts'.length), reached);
  }
  return windows;
}

describe('IPC contract with lib.rs and the capability files', () => {
  let registered: string[];
  let wrappers: Map<string, string>;
  beforeAll(async () => {
    registered = registeredCommands(libSource);
    wrappers = await wrapperCommands();
  });

  it('parses a plausible generate_handler! list', () => {
    expect(registered.length).toBeGreaterThan(20);
    for (const name of registered) expect(name).toMatch(/^[a-z_][a-z0-9_]*$/);
    expect(new Set(registered).size).toBe(registered.length);
  });

  it('registers every command a wrapper invokes', () => {
    const missing = [...wrappers].filter(([command]) => !registered.includes(command)).map(([command, wrapper]) => `${wrapper} -> ${command}`);
    expect(missing).toEqual([]);
  });

  it('gives every registered command a wrapper unless it is a known Rust-only command', () => {
    const unwrapped = registered.filter((command) => !wrappers.has(command) && !REGISTERED_WITHOUT_FRONTEND_CALLER.includes(command));
    expect(unwrapped).toEqual([]);
  });

  it('keeps invoke inside tauri.ts', () => {
    const importsInvoke = /import\s*\{[^}]*\binvoke\b[^}]*\}\s*from\s*['"]@tauri-apps\/api\/core['"]|import\s*\*\s*as\s+\w+\s+from\s*['"]@tauri-apps\/api\/core['"]/;
    expect(Object.keys(sources)).toContain(WRAPPER_FILE);
    const offenders = Object.entries(sources).filter(([path, source]) => path !== WRAPPER_FILE && importsInvoke.test(source)).map(([path]) => path);
    expect(offenders).toEqual([]);
  });

  it('has one entry script for every window the default capability lists', () => {
    const listed = capabilities.find((capability) => capability.identifier === 'default')?.windows ?? [];
    expect([...filesByWindow().keys()].sort()).toEqual([...listed].sort());
  });

  it('forwards console errors to the log in every window', () => {
    for (const [label, files] of filesByWindow()) {
      expect([...files].some((file) => pluginImports(sources[file]).includes('@tauri-apps/plugin-log:error')), label).toBe(true);
    }
  });

  it('grants each plugin call to every window whose code makes it', () => {
    const unknown = new Set<string>();
    const reachedCalls = new Set<string>();
    const denied: string[] = [];
    for (const [label, files] of filesByWindow()) {
      for (const file of files) {
        for (const call of pluginImports(sources[file])) {
          const permissions = PLUGIN_PERMISSIONS[call];
          if (!permissions) { unknown.add(`${call} in ${file}`); continue; }
          reachedCalls.add(call);
          const granted = capabilities.some((capability) =>
            (capability.windows ?? []).some((listed) => listed === label || listed === '*')
            && capability.permissions.some((permission) => permissions.includes(typeof permission === 'string' ? permission : permission.identifier)));
          if (!granted) denied.push(`${call} in the ${label} window (${file})`);
        }
      }
    }
    expect([...unknown]).toEqual([]);
    // A call no window reaches means the table is stale or the import walk missed a file.
    expect(Object.keys(PLUGIN_PERMISSIONS).filter((call) => !reachedCalls.has(call))).toEqual([]);
    expect(denied).toEqual([]);
  });
});
