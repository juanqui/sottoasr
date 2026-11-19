/** Display helpers for Tauri shortcut strings such as "CommandOrControl+Shift+Space". */

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

/** The order macOS shows modifiers in, and the order shortcuts are written in. */
export const MODIFIER_ORDER = ['Control', 'Alt', 'Shift', 'CommandOrControl'];

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

const NAMED_KEYS: Record<string, string> = {
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

export function modifierSymbol(modifier: string): string {
  return MODIFIER_SYMBOLS[modifier] ?? modifier;
}

function keyToDisplay(key: string): string {
  if (KEY_SYMBOLS[key]) return KEY_SYMBOLS[key];
  if (/^Key([A-Z])$/.test(key)) return key.slice(3);
  if (/^Digit(\d)$/.test(key)) return key.slice(5);
  if (/^Numpad(\d)$/.test(key)) return 'Num' + key.slice(6);
  if (/^F\d{1,2}$/.test(key)) return key;
  return NAMED_KEYS[key] ?? key;
}

/** One symbol per modifier, in macOS order, followed by the key. */
export function shortcutKeys(shortcut: string): string[] {
  if (!shortcut) return [];
  const modifiers: string[] = [];
  let key = '';
  for (const part of shortcut.split('+')) {
    if (MODIFIER_SYMBOLS[part]) modifiers.push(part);
    else key = part;
  }
  const rank = (modifier: string) => {
    const index = MODIFIER_ORDER.indexOf(modifier);
    return index === -1 ? 99 : index;
  };
  modifiers.sort((a, b) => rank(a) - rank(b));
  const keys = modifiers.map(modifierSymbol);
  if (key) keys.push(keyToDisplay(key));
  return keys;
}

export function formatShortcut(shortcut: string): string {
  return shortcutKeys(shortcut).join('');
}
