<script lang="ts">
  /**
   * ShortcutRecorder — click-to-record keyboard shortcut input.
   *
   * UX flow:
   * 1. Displays the current shortcut using macOS symbols (⌘⇧Space)
   * 2. Click to enter recording mode ("Press a shortcut...")
   * 3. Press modifier(s) + a non-modifier key to record
   * 4. Escape cancels, Backspace/Delete clears
   * 5. Clicking outside cancels recording
   *
   * Outputs Tauri-compatible shortcut strings like "CommandOrControl+Shift+Space"
   */
  import { onDestroy } from 'svelte';
  import { invoke } from '@tauri-apps/api/core';
  import { listen, type UnlistenFn } from '@tauri-apps/api/event';

  interface Props {
    value: string;
    onchange: (value: string) => void;
    /** If true, this recorder cannot be activated (another is recording) */
    disabled?: boolean;
    /** Called when this recorder enters recording mode */
    onrecordstart?: () => void;
    /** Called when this recorder exits recording mode */
    onrecordend?: () => void;
    placeholder?: string;
  }

  let {
    value,
    onchange,
    disabled = false,
    onrecordstart,
    onrecordend,
    placeholder = 'Click to set',
  }: Props = $props();

  let recording = $state(false);
  let currentModifiers = $state<Set<string>>(new Set());
  let buttonEl: HTMLButtonElement;

  // ---- macOS symbol mappings ----

  const MODIFIER_SYMBOLS: Record<string, string> = {
    Control: '⌃',
    Alt: '⌥',
    Shift: '⇧',
    CommandOrControl: '⌘',
    Command: '⌘',
    CmdOrCtrl: '⌘',
    Super: '⌘',
    Meta: '⌘',
  };

  const MODIFIER_ORDER = ['Control', 'Alt', 'Shift', 'CommandOrControl'];

  const KEY_SYMBOLS: Record<string, string> = {
    Space: '␣',
    Enter: '↩',
    Backspace: '⌫',
    Delete: '⌦',
    Escape: '⎋',
    Tab: '⇥',
    ArrowUp: '↑',
    ArrowDown: '↓',
    ArrowLeft: '←',
    ArrowRight: '→',
    PageUp: '⇞',
    PageDown: '⇟',
    Home: '↖',
    End: '↘',
    CapsLock: '⇪',
  };

  const JS_MODIFIER_KEYS = new Set(['Meta', 'Control', 'Shift', 'Alt']);

  function codeToTauriKey(code: string): string | null {
    if (/^Key[A-Z]$/.test(code)) return code;
    if (/^Digit\d$/.test(code)) return code;
    if (/^F\d{1,2}$/.test(code)) return code;
    if (/^Numpad\d$/.test(code)) return code;
    const map: Record<string, string> = {
      // Standard keys
      Space: 'Space', Enter: 'Enter', Tab: 'Tab', Escape: 'Escape',
      Backspace: 'Backspace', Delete: 'Delete',
      // Navigation
      ArrowUp: 'ArrowUp', ArrowDown: 'ArrowDown',
      ArrowLeft: 'ArrowLeft', ArrowRight: 'ArrowRight',
      Home: 'Home', End: 'End', PageUp: 'PageUp', PageDown: 'PageDown',
      // Modifiers as keys (CapsLock can be used as a shortcut key)
      CapsLock: 'CapsLock',
      // Punctuation
      Minus: 'Minus', Equal: 'Equal',
      BracketLeft: 'BracketLeft', BracketRight: 'BracketRight',
      Backslash: 'Backslash', Semicolon: 'Semicolon', Quote: 'Quote',
      Backquote: 'Backquote', Comma: 'Comma', Period: 'Period', Slash: 'Slash',
      IntlBackslash: 'IntlBackslash',
      // Numpad
      NumpadAdd: 'NumpadAdd', NumpadSubtract: 'NumpadSubtract',
      NumpadMultiply: 'NumpadMultiply', NumpadDivide: 'NumpadDivide',
      NumpadDecimal: 'NumpadDecimal', NumpadEnter: 'NumpadEnter',
      NumpadEqual: 'NumpadEqual',
      NumLock: 'NumLock', ScrollLock: 'ScrollLock',
      // Media keys (detected on some platforms / keyboards)
      MediaPlayPause: 'MediaPlayPause', MediaStop: 'MediaStop',
      MediaTrackNext: 'MediaTrackNext', MediaTrackPrevious: 'MediaTrackPrevious',
      AudioVolumeUp: 'AudioVolumeUp', AudioVolumeDown: 'AudioVolumeDown',
      AudioVolumeMute: 'AudioVolumeMute',
      // Insert, PrintScreen, etc.
      Insert: 'Insert', PrintScreen: 'PrintScreen', Pause: 'Pause',
      ContextMenu: 'ContextMenu',
    };
    return map[code] ?? null;
  }

  function keyToDisplay(key: string): string {
    if (KEY_SYMBOLS[key]) return KEY_SYMBOLS[key];
    if (/^Key([A-Z])$/.test(key)) return key.slice(3);
    if (/^Digit(\d)$/.test(key)) return key.slice(5);
    if (/^Numpad(\d)$/.test(key)) return 'Num' + key.slice(6);
    if (/^F\d{1,2}$/.test(key)) return key;
    const named: Record<string, string> = {
      Minus: '-', Equal: '=', BracketLeft: '[', BracketRight: ']',
      Backslash: '\\', Semicolon: ';', Quote: "'", Backquote: '`',
      Comma: ',', Period: '.', Slash: '/', IntlBackslash: '§',
      NumpadAdd: 'Num+', NumpadSubtract: 'Num-', NumpadMultiply: 'Num*',
      NumpadDivide: 'Num/', NumpadDecimal: 'Num.', NumpadEnter: 'NumEnter',
      NumpadEqual: 'Num=', NumLock: 'NumLock', ScrollLock: 'ScrollLock',
      MediaPlayPause: '⏯', MediaStop: '⏹', MediaTrackNext: '⏭',
      MediaTrackPrevious: '⏮', AudioVolumeUp: '🔊', AudioVolumeDown: '🔉',
      AudioVolumeMute: '🔇', Insert: 'Ins', PrintScreen: 'PrtSc',
      Pause: 'Pause', ContextMenu: 'Menu',
    };
    return named[key] ?? key;
  }

  function formatForDisplay(shortcut: string): string {
    if (!shortcut) return '';
    const parts = shortcut.split('+');
    const modifiers: string[] = [];
    let key = '';
    for (const part of parts) {
      if (MODIFIER_SYMBOLS[part]) {
        modifiers.push(part);
      } else {
        key = part;
      }
    }
    modifiers.sort((a, b) => {
      const ai = MODIFIER_ORDER.indexOf(a);
      const bi = MODIFIER_ORDER.indexOf(b);
      return (ai === -1 ? 99 : ai) - (bi === -1 ? 99 : bi);
    });
    const symbolParts = modifiers.map(m => MODIFIER_SYMBOLS[m] || m);
    if (key) symbolParts.push(keyToDisplay(key));
    return symbolParts.join('');
  }

  function formatActiveModifiers(): string {
    const parts: string[] = [];
    for (const mod of MODIFIER_ORDER) {
      if (currentModifiers.has(mod)) {
        parts.push(MODIFIER_SYMBOLS[mod] || mod);
      }
    }
    return parts.length > 0 ? parts.join('') + '...' : 'Press a shortcut...';
  }

  // ---- Key capture via Rust CGEventTap + JS fallback ----

  let keyCaptureUnlisten: UnlistenFn | null = null;
  let modifierUnlisten: UnlistenFn | null = null;

  function handleCapturedKey(payload: { code: string; key: string; source: string; metaKey?: boolean; shiftKey?: boolean; altKey?: boolean; ctrlKey?: boolean }) {
    if (!recording) return;

    const code = payload.code;

    if (code === 'Escape') {
      stopRecording();
      return;
    }

    if (code === 'Backspace' || code === 'Delete') {
      if (!payload.metaKey && !payload.ctrlKey && !payload.altKey && !payload.shiftKey) {
        onchange('');
        stopRecording();
        return;
      }
    }

    // Build modifier set
    const mods = new Set<string>();
    if (payload.metaKey) mods.add('CommandOrControl');
    if (payload.ctrlKey && !payload.metaKey) mods.add('Control');
    if (payload.altKey) mods.add('Alt');
    if (payload.shiftKey) mods.add('Shift');
    currentModifiers = mods;

    // For system-level keys (media, brightness), accept without modifiers
    const isSystemKey = payload.source === 'system';

    if (mods.size === 0 && !isSystemKey) {
      if (!/^F\d{1,2}$/.test(code)) {
        return;
      }
    }

    // Use the code directly — Rust already maps to Tauri-compatible names
    const parts: string[] = [];
    for (const mod of MODIFIER_ORDER) {
      if (mods.has(mod)) parts.push(mod);
    }
    parts.push(code);

    onchange(parts.join('+'));
    stopRecording();
  }

  function handleModifierUpdate(payload: { metaKey: boolean; shiftKey: boolean; altKey: boolean; ctrlKey: boolean }) {
    if (!recording) return;
    const mods = new Set<string>();
    if (payload.metaKey) mods.add('CommandOrControl');
    if (payload.ctrlKey && !payload.metaKey) mods.add('Control');
    if (payload.altKey) mods.add('Alt');
    if (payload.shiftKey) mods.add('Shift');
    currentModifiers = mods;
  }

  function handleDocMouseDown(event: MouseEvent) {
    if (buttonEl && !buttonEl.contains(event.target as Node)) {
      stopRecording();
    }
  }

  // Also keep a JS keydown handler as fallback (in case CGEventTap can't start)
  function handleDocKeyDown(event: KeyboardEvent) {
    event.preventDefault();
    event.stopPropagation();
    if (event.repeat) return;

    handleCapturedKey({
      code: codeToTauriKey(event.code) || event.code || event.key,
      key: event.key,
      source: 'js-fallback',
      metaKey: event.metaKey,
      shiftKey: event.shiftKey,
      altKey: event.altKey,
      ctrlKey: event.ctrlKey,
    });
  }

  async function startRecording() {
    if (disabled) return;
    recording = true;
    currentModifiers = new Set();
    onrecordstart?.();

    // Start Rust-side CGEventTap for full key capture
    try {
      await invoke('start_key_capture');
      keyCaptureUnlisten = await listen<any>('key-captured', (event) => {
        handleCapturedKey(event.payload);
      });
      modifierUnlisten = await listen<any>('key-modifier', (event) => {
        handleModifierUpdate(event.payload);
      });
    } catch (e) {
      console.warn('CGEventTap not available, using JS fallback:', e);
    }

    // JS fallback (always active as backup)
    document.addEventListener('keydown', handleDocKeyDown, true);
    document.addEventListener('mousedown', handleDocMouseDown, true);
  }

  async function stopRecording() {
    if (!recording) return;
    recording = false;
    currentModifiers = new Set();

    // Stop Rust-side capture
    try {
      await invoke('stop_key_capture');
    } catch {}
    keyCaptureUnlisten?.();
    keyCaptureUnlisten = null;
    modifierUnlisten?.();
    modifierUnlisten = null;

    // Remove JS listeners
    document.removeEventListener('keydown', handleDocKeyDown, true);
    document.removeEventListener('mousedown', handleDocMouseDown, true);
    onrecordend?.();
  }

  onDestroy(() => {
    if (recording) {
      invoke('stop_key_capture').catch(() => {});
    }
    keyCaptureUnlisten?.();
    modifierUnlisten?.();
    document.removeEventListener('keydown', handleDocKeyDown, true);
    document.removeEventListener('mousedown', handleDocMouseDown, true);
  });

  let displayText = $derived(
    recording
      ? formatActiveModifiers()
      : value
        ? formatForDisplay(value)
        : placeholder
  );
</script>

<button
  bind:this={buttonEl}
  class="shortcut-recorder"
  class:recording
  class:disabled
  class:empty={!value && !recording}
  onclick={() => { if (!recording && !disabled) startRecording(); }}
  type="button"
  role="textbox"
  aria-label="Shortcut recorder"
>
  <span class="shortcut-display">{displayText}</span>
  {#if value && !recording}
    <button
      class="clear-btn"
      onclick={(e) => { e.stopPropagation(); onchange(''); }}
      type="button"
      aria-label="Clear shortcut"
      tabindex={-1}
    >×</button>
  {/if}
</button>

<style>
  .shortcut-recorder {
    display: flex;
    align-items: center;
    justify-content: space-between;
    width: 100%;
    padding: 8px 12px;
    border: 1px solid var(--border);
    border-radius: 8px;
    background: var(--input-bg);
    color: var(--text-bright);
    font-size: 14px;
    font-family: -apple-system, BlinkMacSystemFont, 'SF Pro Text', system-ui, sans-serif;
    cursor: pointer;
    outline: none;
    transition: border-color 0.15s ease, box-shadow 0.15s ease;
    min-height: 36px;
    box-sizing: border-box;
    text-align: left;
    letter-spacing: 0.5px;
  }

  .shortcut-recorder:focus {
    border-color: var(--accent);
  }

  .shortcut-recorder.recording {
    border-color: var(--accent);
    box-shadow: 0 0 0 2px rgba(59, 130, 246, 0.25);
    animation: pulse-border 1.5s ease-in-out infinite;
  }

  .shortcut-recorder.disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .shortcut-recorder.empty {
    color: var(--text-dim);
  }

  @keyframes pulse-border {
    0%, 100% {
      box-shadow: 0 0 0 2px rgba(59, 130, 246, 0.25);
    }
    50% {
      box-shadow: 0 0 0 3px rgba(59, 130, 246, 0.15);
    }
  }

  .shortcut-display {
    flex: 1;
    pointer-events: none;
  }

  .shortcut-recorder.recording .shortcut-display {
    color: var(--accent);
    font-style: italic;
  }

  .clear-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 18px;
    height: 18px;
    border: none;
    border-radius: 50%;
    background: var(--border);
    color: var(--text-dim);
    font-size: 13px;
    line-height: 1;
    cursor: pointer;
    padding: 0;
    flex-shrink: 0;
    transition: background 0.15s ease, color 0.15s ease;
  }

  .clear-btn:hover {
    background: var(--border-hover);
    color: var(--text);
  }
</style>
