// Passed directly to Playwright addInitScript: keep this function self-contained.
// All content is synthetic; no native IPC, real history, or model is accessed.
export function installFixture({ historyCount = 0, dictionaryCount = 0, label = 'settings' } = {}) {
  const callbacks = new Map();
  const events = new Map();
  let nextId = 1;
  const settings = {
    push_to_talk_shortcut: 'CommandOrControl+Shift+Space',
    toggle_shortcut: 'CommandOrControl+Shift+D',
    cancel_shortcut: 'Escape', open_settings_shortcut: 'CommandOrControl+Shift+Comma',
    show_overlay: true, auto_paste: true, restore_clipboard: true, restore_focus_before_paste: true,
    model_path: '', language: 'auto', max_history: 500, launch_at_login: false,
    llm_cleanup_enabled: false, auto_check_updates: true, vocabulary: [],
    dictionary: Array.from({ length: dictionaryCount }, (_, index) => ({
      heard: `Misspelled ${index}`, replacement: `Preferred ${index}`,
    })),
  };
  const paragraph = 'We are testing a synthetic transcript about local tools and project settings. Keep every sentence and number 8472 in order. ';
  const history = Array.from({ length: historyCount }, (_, index) => ({
    id: `fixture-${index}`, text: `${paragraph.repeat(6)} Item ${index}.`,
    raw_text: index % 2 ? `${paragraph.repeat(6)} Itam ${index}.` : undefined,
    created_at: '2026-09-08T06:00:00Z', duration_ms: 60000, word_count: 140,
    llm_applied: false, llm_cleanup_status: { kind: 'disabled' },
  }));
  window.__uiBench = { ipc: [], unknownCommands: [], errors: [], longTasksMs: [], raf: 0, draw: 0 };
  const metrics = window.__uiBench;
  let overlay = { revision: 0, generation: 0, state: 'Idle', started_at_ms: null, error: null };
  window.addEventListener('error', (event) => metrics.errors.push(event.message));
  window.addEventListener('unhandledrejection', (event) => metrics.errors.push(String(event.reason)));
  window.__emit = (event, payload) => {
    if (event === 'overlay-state') overlay = structuredClone(payload);
    for (const listener of events.values()) {
      if (listener.event === event) callbacks.get(listener.handler)?.({ event, payload });
    }
  };
  const raf = window.requestAnimationFrame.bind(window);
  window.requestAnimationFrame = (callback) => { metrics.raf += 1; return raf(callback); };
  const roundRect = CanvasRenderingContext2D.prototype.roundRect;
  CanvasRenderingContext2D.prototype.roundRect = function (...args) {
    metrics.draw += 1;
    return roundRect.apply(this, args);
  };
  if (PerformanceObserver.supportedEntryTypes.includes('longtask')) {
    new PerformanceObserver((list) => metrics.longTasksMs.push(...list.getEntries().map((item) => item.duration)))
      .observe({ type: 'longtask', buffered: true });
  }
  window.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener: (_, id) => events.delete(id) };
  window.__TAURI_INTERNALS__ = {
    metadata: { currentWindow: { label }, currentWebview: { label } },
    transformCallback: (callback) => { const id = nextId++; callbacks.set(id, callback); return id; },
    unregisterCallback: (id) => callbacks.delete(id),
    invoke: async (command, args = {}) => {
      metrics.ipc.push({ command, at: performance.now() });
      if (command === 'plugin:event|listen') { const id = nextId++; events.set(id, args); return id; }
      if (command === 'plugin:event|unlisten') { events.delete(args.eventId); return; }
      if (command === 'get_settings') return structuredClone(settings);
      if (command === 'update_settings') {
        Object.assign(settings, args.newSettings);
        return { settings: structuredClone(settings), warnings: [] };
      }
      if (command === 'get_vocabulary_status') return {
        supported: true, downloaded: false, loaded: false, preparing: false, download_size_mb: 103, error: null,
      };
      if (command === 'get_transcriptions') return structuredClone(history);
      if (command === 'get_overlay_snapshot') return structuredClone(overlay);
      if (command === 'get_llm_status') return {
        available: true, unavailable_reason: null, downloaded: false, downloading: false,
        loaded: false, preparing: false, setup_error: null,
        model_name: 'Synthetic cleanup model', model_url: 'https://example.invalid/local-model', download_size_mb: 227,
        update_available: false, last_cleanup_status: { kind: 'idle' },
      };
      if (command === 'check_all_permissions') return {
        microphone: 'authorized', accessibility_api: true, accessibility_functional: true, needs_restart: false,
      };
      if (command === 'check_llm_update') return false;
      metrics.unknownCommands.push(command);
      throw new Error(`UI benchmark has no fixture for command: ${command}`);
    },
  };
}
