import { error as logError, warn as logWarn } from '@tauri-apps/plugin-log';

function describeArg(value: unknown): string {
  if (typeof value === 'string') return value;
  if (value instanceof Error) return `${value.name}: ${value.message}`;
  try { return JSON.stringify(value) ?? String(value); } catch { return String(value); }
}

/**
 * Copy console.error and console.warn into the app log file; the web inspector
 * still shows them. Every entry script calls this once, before it mounts.
 * Console calls must never pass transcript text, clipboard text, or key names.
 */
export function forwardConsoleToLog() {
  for (const [level, write] of [['error', logError], ['warn', logWarn]] as const) {
    const original = console[level].bind(console);
    console[level] = (...args: unknown[]) => {
      original(...args);
      // The inspector already has the message; a failed log write has nowhere better to go.
      write(args.map(describeArg).join(' ')).catch(() => {});
    };
  }
}
