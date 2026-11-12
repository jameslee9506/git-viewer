// Small shared formatting helpers. Keep output terse (minimal-text UI).

export function shortOid(oid: string, n = 7): string {
  return oid.slice(0, n);
}

const MIN = 60;
const HOUR = 3600;
const DAY = 86400;

/** Compact relative time: now, 5m, 3h, 2d, 3w, 4mo, 2y. */
export function relativeTime(unix: number, now = Date.now() / 1000): string {
  const d = Math.max(0, now - unix);
  if (d < MIN) return 'now';
  if (d < HOUR) return `${Math.floor(d / MIN)}m`;
  if (d < DAY) return `${Math.floor(d / HOUR)}h`;
  if (d < DAY * 7) return `${Math.floor(d / DAY)}d`;
  if (d < DAY * 30) return `${Math.floor(d / (DAY * 7))}w`;
  if (d < DAY * 365) return `${Math.floor(d / (DAY * 30))}mo`;
  return `${Math.floor(d / (DAY * 365))}y`;
}

const pad = (n: number) => String(n).padStart(2, '0');

/** Local time `YYYY-MM-DD HH:mm`. */
export function absoluteTime(unix: number): string {
  const t = new Date(unix * 1000);
  return `${t.getFullYear()}-${pad(t.getMonth() + 1)}-${pad(t.getDate())} ${pad(t.getHours())}:${pad(t.getMinutes())}`;
}

/** Local date only `YYYY-MM-DD`. */
export function absoluteDate(unix: number): string {
  const t = new Date(unix * 1000);
  return `${t.getFullYear()}-${pad(t.getMonth() + 1)}-${pad(t.getDate())}`;
}

/** Local time of day `HH:mm`. */
export function clockTime(unix: number): string {
  const t = new Date(unix * 1000);
  return `${pad(t.getHours())}:${pad(t.getMinutes())}`;
}

/** The local calendar day of `unix`, as a number that tells days apart (yyyymmdd-like, not a date). */
export function dayKey(unix: number): number {
  const t = new Date(unix * 1000);
  return t.getFullYear() * 10000 + t.getMonth() * 100 + t.getDate();
}

const MONTHS = ['Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', 'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec'];
const WEEKDAYS = ['Sunday', 'Monday', 'Tuesday', 'Wednesday', 'Thursday', 'Friday', 'Saturday'];

/**
 * The local day of `unix` as a heading: Today, Yesterday, the weekday within the week before, else
 * `Nov 3` (this year) or `Nov 3, 2025`. Days are counted between local midnights (a DST day is 23 or 25
 * hours long: rounded).
 */
export function dayLabel(unix: number, now = Date.now() / 1000): string {
  const t = new Date(unix * 1000);
  const today = new Date(now * 1000);
  const midnight = (d: Date) => new Date(d.getFullYear(), d.getMonth(), d.getDate()).getTime();
  const days = Math.round((midnight(today) - midnight(t)) / (DAY * 1000));
  if (days === 0) return 'Today';
  if (days === 1) return 'Yesterday';
  if (days > 1 && days < 7) return WEEKDAYS[t.getDay()];
  const md = `${MONTHS[t.getMonth()]} ${t.getDate()}`;
  return t.getFullYear() === today.getFullYear() ? md : `${md}, ${t.getFullYear()}`;
}

export function formatTime(unix: number, mode: 'relative' | 'absolute'): string {
  if (!unix) return '';
  return mode === 'relative' ? relativeTime(unix) : absoluteTime(unix);
}

/** Time in the committer's own zone, e.g. `2026-11-08 10:40 +0900`. */
export function timeWithZone(unix: number, tz: string): string {
  const m = /^([+-])(\d{2})(\d{2})$/.exec(tz);
  if (!m) return absoluteTime(unix);
  const sign = m[1] === '-' ? -1 : 1;
  const offset = sign * (Number(m[2]) * 3600 + Number(m[3]) * 60);
  const t = new Date((unix + offset) * 1000);
  return `${t.getUTCFullYear()}-${pad(t.getUTCMonth() + 1)}-${pad(t.getUTCDate())} ${pad(t.getUTCHours())}:${pad(t.getUTCMinutes())} ${tz}`;
}

export function basename(path: string): string {
  const i = path.lastIndexOf('/');
  return i < 0 ? path : path.slice(i + 1);
}

export function dirname(path: string): string {
  const i = path.lastIndexOf('/');
  return i < 0 ? '' : path.slice(0, i);
}

export function extname(path: string): string {
  const b = basename(path);
  const i = b.lastIndexOf('.');
  return i <= 0 ? '' : b.slice(i + 1).toLowerCase();
}

/** Collapse $HOME to ~ for display. */
export function tildify(path: string, home: string | null): string {
  if (home && (path === home || path.startsWith(home + '/'))) return '~' + path.slice(home.length);
  return path;
}

const IMAGE_EXT = new Set(['png', 'jpg', 'jpeg', 'gif', 'webp', 'bmp', 'ico', 'svg', 'avif', 'tiff', 'tif', 'heic']);

export function isImagePath(path: string): boolean {
  return IMAGE_EXT.has(extname(path));
}

export function formatBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  return `${(n / (1024 * 1024)).toFixed(1)} MB`;
}

/** Copy text to the clipboard (WKWebView supports navigator.clipboard on user gestures). */
export async function copyText(text: string): Promise<void> {
  try {
    await navigator.clipboard.writeText(text);
  } catch {
    const ta = document.createElement('textarea');
    ta.value = text;
    ta.style.position = 'fixed';
    ta.style.opacity = '0';
    document.body.appendChild(ta);
    ta.select();
    document.execCommand('copy');
    ta.remove();
  }
}
