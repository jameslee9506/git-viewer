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

export function initials(name: string): string {
  const parts = name.trim().split(/\s+/).filter(Boolean);
  if (parts.length === 0) return '?';
  if (parts.length === 1) return parts[0].slice(0, 2).toUpperCase();
  return (parts[0][0] + parts[parts.length - 1][0]).toUpperCase();
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
