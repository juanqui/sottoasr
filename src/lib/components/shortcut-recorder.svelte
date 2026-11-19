<script lang="ts">
  /**
   * ShortcutRecorder — click-to-record keyboard shortcut input.
   *
   * UX flow:
   * 1. Displays the current shortcut using macOS symbols (⌘⇧Space)
   * 2. Click to enter recording mode ("Press a shortcut...")
   * 3. Press modifier(s) + a non-modifier key to record
   * 4. Escape cancels; Backspace/Delete clears an optional shortcut
   * 5. Clicking outside cancels recording
   *
   * A required shortcut (one with `defaultValue`) cannot be cleared; it offers
   * "Reset to default" instead. That is also the only way back to Escape,
   * because pressing Escape cancels recording.
   *
   * Outputs Tauri-compatible shortcut strings like "CommandOrControl+Shift+Space"
   */
  import { onDestroy } from 'svelte';
  import { startKeyCapture, stopKeyCapture } from '../utils/tauri';
  import { createEventScope } from '../utils/event-scope';
  import { MODIFIER_ORDER, formatShortcut, modifierSymbol } from './shortcut-format';

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
    label?: string;
    /** Marks the shortcut as required and gives the value "Reset to default" restores. */
    defaultValue?: string;
  }

  let {
    value,
    onchange,
    disabled = false,
    onrecordstart,
    onrecordend,
    placeholder = 'Click to set',
    label = 'Shortcut recorder',
    defaultValue,
  }: Props = $props();

  let recording = $state(false);
  let currentModifiers = $state<Set<string>>(new Set());
  let pendingShortcut = $state('');
  let commitTimeoutId: ReturnType<typeof setTimeout> | null = null;
  let buttonEl: HTMLButtonElement;

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

  function formatActiveModifiers(): string {
    const parts: string[] = [];
    for (const mod of MODIFIER_ORDER) {
      if (currentModifiers.has(mod)) {
        parts.push(modifierSymbol(mod));
      }
    }
    return parts.length > 0 ? parts.join('') + '...' : 'Press a shortcut...';
  }

  // ---- Key capture via Rust CGEventTap + JS fallback ----

  let captureScope: ReturnType<typeof createEventScope> | null = null;
  let captureGeneration = 0;
  let disposed = false;

  function handleCapturedKey(payload: { code: string; key: string; source: string; metaKey?: boolean; shiftKey?: boolean; altKey?: boolean; ctrlKey?: boolean }) {
    if (!recording) return;

    const code = payload.code;

    if (code === 'Escape') {
      pendingShortcut = '';
      stopRecording();
      return;
    }

    // A required shortcut cannot be cleared; a bare Backspace or Delete is
    // then ignored like any other key without a modifier.
    if ((code === 'Backspace' || code === 'Delete') && defaultValue === undefined) {
      if (!payload.metaKey && !payload.ctrlKey && !payload.altKey && !payload.shiftKey) {
        pendingShortcut = '';
        onchange('');
        stopRecording();
        return;
      }
    }

    // Build modifier set
    const mods = new Set<string>();
    if (payload.metaKey) mods.add('CommandOrControl');
    if (payload.ctrlKey) mods.add('Control');
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

    pendingShortcut = parts.join('+');

    // For shortcuts without modifiers (system/function keys), commit immediately
    if (mods.size === 0) {
      commitPending();
      return;
    }

    // For shortcuts with modifiers, wait for all keys to be released.
    // Set a fallback timeout in case release detection fails.
    clearCommitTimeout();
    commitTimeoutId = setTimeout(commitPending, 3000);
  }

  function handleModifierUpdate(payload: { metaKey: boolean; shiftKey: boolean; altKey: boolean; ctrlKey: boolean }) {
    if (!recording) return;
    const mods = new Set<string>();
    if (payload.metaKey) mods.add('CommandOrControl');
    if (payload.ctrlKey) mods.add('Control');
    if (payload.altKey) mods.add('Alt');
    if (payload.shiftKey) mods.add('Shift');
    currentModifiers = mods;

    // If we have a pending shortcut and all modifiers are released, commit
    if (pendingShortcut && !payload.metaKey && !payload.shiftKey && !payload.altKey && !payload.ctrlKey) {
      commitPending();
    }
  }

  function commitPending() {
    clearCommitTimeout();
    if (pendingShortcut) {
      const shortcut = pendingShortcut;
      pendingShortcut = '';
      onchange(shortcut);
      stopRecording();
    }
  }

  function clearCommitTimeout() {
    if (commitTimeoutId !== null) {
      clearTimeout(commitTimeoutId);
      commitTimeoutId = null;
    }
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

    // For modifier-only keys, just update the modifier display
    if (JS_MODIFIER_KEYS.has(event.key)) {
      handleModifierUpdate({
        metaKey: event.metaKey,
        shiftKey: event.shiftKey,
        altKey: event.altKey,
        ctrlKey: event.ctrlKey,
      });
      return;
    }

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

  // JS keyup handler for release detection
  function handleDocKeyUp(event: KeyboardEvent) {
    if (!recording || !pendingShortcut) return;
    // When all modifier keys are released, commit the pending shortcut
    if (!event.metaKey && !event.shiftKey && !event.altKey && !event.ctrlKey) {
      commitPending();
    }
  }

  async function startRecording() {
    if (disabled || recording || disposed) return;
    const generation = ++captureGeneration;
    recording = true;
    currentModifiers = new Set();
    pendingShortcut = '';
    clearCommitTimeout();
    onrecordstart?.();
    const scope = createEventScope();
    captureScope = scope;
    // Install the JS fallback immediately, so Escape/outside clicks work even
    // while native event registration is still pending.
    document.addEventListener('keydown', handleDocKeyDown, true);
    document.addEventListener('keyup', handleDocKeyUp, true);
    document.addEventListener('mousedown', handleDocMouseDown, true);
    window.addEventListener('blur', stopRecording);
    await Promise.all([
      scope.listen<Parameters<typeof handleCapturedKey>[0]>('key-captured', (event) => handleCapturedKey(event.payload)),
      scope.listen<Parameters<typeof handleModifierUpdate>[0]>('key-modifier', (event) => handleModifierUpdate(event.payload)),
    ]);
    if (!recording || disposed || generation !== captureGeneration) return;
    try { await startKeyCapture(); }
    catch (error) { console.warn('Native key capture unavailable, using keyboard fallback:', error); }
  }

  function stopRecording() {
    if (!recording) return;
    ++captureGeneration;
    recording = false;
    currentModifiers = new Set();
    pendingShortcut = '';
    clearCommitTimeout();
    captureScope?.dispose();
    captureScope = null;
    document.removeEventListener('keydown', handleDocKeyDown, true);
    document.removeEventListener('keyup', handleDocKeyUp, true);
    document.removeEventListener('mousedown', handleDocMouseDown, true);
    window.removeEventListener('blur', stopRecording);
    onrecordend?.();
    void stopKeyCapture().catch(() => {});
  }

  onDestroy(() => {
    disposed = true;
    stopRecording();
    clearCommitTimeout();
    captureScope?.dispose();
  });

  let displayText = $derived(
    recording
      ? (pendingShortcut ? formatShortcut(pendingShortcut) : formatActiveModifiers())
      : value
        ? formatShortcut(value)
        : placeholder
  );
  let showReset = $derived(defaultValue !== undefined && value !== defaultValue && !recording);

  function resetToDefault() {
    if (defaultValue !== undefined) onchange(defaultValue);
  }
</script>

<div class="shortcut-control">
  <button
    bind:this={buttonEl}
    class="shortcut-recorder"
    class:recording
    class:disabled
    class:empty={!value && !recording}
    class:has-reset={showReset}
    onclick={() => { if (!recording && !disabled) startRecording(); }}
    type="button"
    {disabled}
    aria-label={label}
  >
    <span class="shortcut-display">{displayText}</span>
  </button>
  {#if defaultValue !== undefined}
    {#if showReset}
      <button class="reset-btn" onclick={resetToDefault} type="button" {disabled}>Reset to default</button>
    {/if}
  {:else if value && !recording}
    <button
      class="clear-btn"
      onclick={() => onchange('')}
      type="button"
      aria-label={`Clear ${label.toLowerCase()}`}
      {disabled}
    >×</button>
  {/if}
</div>

<style>
  .shortcut-control {
    position: relative;
    flex: 1;
    min-width: 0;
  }

  .shortcut-recorder {
    display: flex;
    align-items: center;
    justify-content: space-between;
    width: 100%;
    padding: 8px 38px 8px 12px;
    border: 1px solid var(--border);
    border-radius: 8px;
    background: var(--input-bg);
    color: var(--text-bright);
    font-size: 14px;
    font-family: var(--sans);
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

  .shortcut-recorder.has-reset {
    padding-right: 124px;
  }

  .shortcut-recorder.recording {
    border-color: var(--accent);
    box-shadow: 0 0 0 2px var(--selection-bg);
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
      box-shadow: 0 0 0 2px var(--selection-bg);
    }
    50% {
      box-shadow: 0 0 0 3px var(--accent-bg);
    }
  }

  .shortcut-display {
    flex: 1;
    pointer-events: none;
  }

  .shortcut-recorder.recording .shortcut-display {
    color: var(--accent-text);
    font-style: italic;
  }

  .clear-btn {
    position: absolute;
    right: 12px;
    top: 50%;
    transform: translateY(-50%);
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

  .reset-btn {
    position: absolute;
    right: 8px;
    top: 50%;
    transform: translateY(-50%);
    border: none;
    border-radius: 6px;
    background: var(--hover-bg);
    color: var(--accent-text);
    font: inherit;
    font-size: 11px;
    line-height: 1;
    padding: 5px 7px;
    cursor: pointer;
    white-space: nowrap;
  }

  .reset-btn:hover:not(:disabled) {
    background: var(--accent-bg);
  }

  .reset-btn:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }
</style>
