// Keyboard helpers. Shortcuts match on `e.code` (the physical key): with the Korean input source
// `e.key` yields jamo. Native-menu accelerators never reach keydown.

const IS_MAC = typeof navigator !== 'undefined' && /Mac|iPhone|iPad/.test(navigator.userAgent);

const TEXT_INPUT_TYPES = new Set([
  'text',
  'search',
  'url',
  'tel',
  'email',
  'password',
  'number',
  'date',
  'datetime-local',
  'month',
  'time',
  'week',
]);

/** True when keys typed at `target` belong to a text field (input, textarea, select, contenteditable). */
export function isEditable(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) return false;
  if (target.isContentEditable) return true;
  switch (target.tagName) {
    case 'TEXTAREA':
    case 'SELECT':
      return true;
    case 'INPUT':
      return TEXT_INPUT_TYPES.has((target as HTMLInputElement).type);
    default:
      return false;
  }
}

/** An IME composition is in progress (Korean/Japanese input): the key belongs to the composer. */
export function isComposing(e: KeyboardEvent): boolean {
  return e.isComposing || e.keyCode === 229;
}

/** True when a keydown must not trigger shortcuts: IME composition or typing into a text field. */
export function ignoreKey(e: KeyboardEvent): boolean {
  return isComposing(e) || isEditable(e.target);
}

/** `e.code` equality: 'KeyJ', 'BracketLeft', 'Digit1', 'ArrowDown', 'Escape', … */
export function codeIs(e: KeyboardEvent, code: string): boolean {
  return e.code === code;
}

/** The primary shortcut modifier is held: ⌘ on macOS, Ctrl elsewhere. Also for mouse events (⌘-click). */
export function modKey(e: { metaKey: boolean; ctrlKey: boolean }): boolean {
  return IS_MAC ? e.metaKey : e.ctrlKey;
}

interface Combo {
  code: string;
  meta: boolean;
  ctrl: boolean;
  alt: boolean;
  shift: boolean;
}

/** Key names accepted in specs besides real `e.code` values. */
const CODE_ALIASES: Record<string, string> = {
  '[': 'BracketLeft',
  ']': 'BracketRight',
  ',': 'Comma',
  '.': 'Period',
  '/': 'Slash',
  '\\': 'Backslash',
  ';': 'Semicolon',
  "'": 'Quote',
  '`': 'Backquote',
  '=': 'Equal',
  '-': 'Minus',
  esc: 'Escape',
  up: 'ArrowUp',
  down: 'ArrowDown',
  left: 'ArrowLeft',
  right: 'ArrowRight',
  pageup: 'PageUp',
  pagedown: 'PageDown',
  space: 'Space',
  enter: 'Enter',
  tab: 'Tab',
};

function toCode(key: string): string {
  if (/^[a-z]$/i.test(key)) return `Key${key.toUpperCase()}`;
  if (/^[0-9]$/.test(key)) return `Digit${key}`;
  return CODE_ALIASES[key.toLowerCase()] ?? CODE_ALIASES[key] ?? key;
}

const combos = new Map<string, Combo>();

function parse(spec: string): Combo {
  const hit = combos.get(spec);
  if (hit) return hit;
  const parts = spec.split('+');
  const c: Combo = { code: toCode(parts.pop() ?? ''), meta: false, ctrl: false, alt: false, shift: false };
  for (const p of parts) {
    switch (p.toLowerCase()) {
      case 'mod':
        if (IS_MAC) c.meta = true;
        else c.ctrl = true;
        break;
      case 'cmd':
      case 'command':
      case 'meta':
        c.meta = true;
        break;
      case 'ctrl':
      case 'control':
        c.ctrl = true;
        break;
      case 'alt':
      case 'opt':
      case 'option':
        c.alt = true;
        break;
      case 'shift':
        c.shift = true;
        break;
      default:
        // Unknown modifier: the spec can never match (a typo must not fire on the bare key).
        c.code = '';
        console.warn(`keys: unknown modifier "${p}" in "${spec}"`);
    }
  }
  combos.set(spec, c);
  return c;
}

/**
 * Exact shortcut match: the key (an `e.code`, or a simple alias like `j`, `]`, `esc`) with exactly the
 * listed modifiers held. Several specs = any of them. Modifiers: mod (⌘ on macOS), cmd, ctrl, alt/opt,
 * shift. Examples: `matches(e, 'mod+shift+BracketRight')`, `matches(e, 'KeyJ', 'ArrowDown')`.
 */
export function matches(e: KeyboardEvent, ...specs: string[]): boolean {
  for (const spec of specs) {
    const c = parse(spec);
    if (
      c.code !== '' &&
      e.code === c.code &&
      e.metaKey === c.meta &&
      e.ctrlKey === c.ctrl &&
      e.altKey === c.alt &&
      e.shiftKey === c.shift
    ) {
      return true;
    }
  }
  return false;
}
