// Syntax highlighting client — the only highlight module the app imports. shiki lives in a lazily created module
// worker; results are kept per document in an LRU (packed) and expanded to Token[][] on demand.
import type { FromWorker, HighlightJob, PackedTokens, ToWorker } from './highlight.worker';

export { languageForPath } from './languages';

/** [text, color] — color is a CSS color, usually `var(--shiki-token-…)`; '' = plain. */
export type Token = [text: string, color: string];

const MAX_LINES = 30_000;
const MAX_CHARS = 2_000_000;
/** Average line length above which a document counts as minified. */
const MAX_AVG_LINE = 500;
const CACHE_DOCS = 60;
/** The cached documents also stay under this many characters in total. */
const CACHE_CHARS = 8_000_000;
/** Only the newest requests stay alive; older ones resolve null (callers ignore stale results anyway). */
const MAX_LIVE = 4;
/** Pending work without any message from the worker for this long → the worker is replaced. */
const WATCHDOG_MS = 5000;
/** Consecutive worker failures after which highlighting is switched off. */
const MAX_FAILURES = 3;
/** Superseded jobs remembered for blaming a hang on. */
const MAX_DROPPED = 16;

interface Entry {
  /** The lines the tokens slice; empty for a null entry. */
  lines: string[];
  data: PackedTokens | null;
  chars: number;
  view: WeakRef<Token[][]> | null;
}

interface Pending {
  job: HighlightJob;
  key: string;
  chars: number;
  promise: Promise<Token[][] | null>;
  resolve: (tokens: Token[][] | null) => void;
}

const cache = new Map<string, Entry>();
const pending = new Map<number, Pending>();
const byKey = new Map<string, Pending>();
/** Keys of recently superseded jobs by id: the worker may already be stuck in one of them. */
const dropped = new Map<number, string>();
let cacheChars = 0;
let worker: Worker | null = null;
let seq = 0;
let busy = 0;
let failures = 0;
let disabled = false;
let watchdog: ReturnType<typeof setTimeout> | undefined;

/**
 * Tokenize `lines` as one document. `resets` = line indexes where the grammar restarts (e.g. hunk starts of a
 * reconstructed diff side). Resolves null if unsupported, too big, failed or superseded. A line's tokens cover its
 * text exactly, minus a trailing '\r'. Results are shared between callers: do not mutate them.
 */
export function highlightLines(lang: string, lines: string[], resets?: number[]): Promise<Token[][] | null> {
  const n = lines.length;
  if (n === 0) return Promise.resolve([]);
  if (disabled || !lang || n > MAX_LINES) return Promise.resolve(null);
  const copy = new Array<string>(n);
  let chars = 0;
  for (let i = 0; i < n; i++) {
    const line = lines[i];
    copy[i] = line;
    chars += line.length;
  }
  if (chars > MAX_CHARS || chars > n * MAX_AVG_LINE) return Promise.resolve(null);
  const cuts = normalizeResets(resets, n);
  const key = `${lang}\0${n}\0${chars}\0${cuts.length}\0${hashDoc(copy, cuts)}`;
  const hit = cache.get(key);
  if (hit) {
    cache.delete(key);
    cache.set(key, hit);
    return Promise.resolve(expand(hit));
  }
  const same = byKey.get(key);
  if (same) {
    // Wanted again: it now counts as the newest request, so it is not the next one superseded.
    pending.delete(same.job.id);
    pending.set(same.job.id, same);
    return same.promise;
  }

  let resolve!: (tokens: Token[][] | null) => void;
  const promise = new Promise<Token[][] | null>((r) => (resolve = r));
  const req: Pending = { job: { id: ++seq, lang, lines: copy, resets: cuts }, key, chars, promise, resolve };
  const idle = pending.size === 0;
  pending.set(req.job.id, req);
  byKey.set(key, req);
  if (!send(req.job)) return promise;
  if (idle) arm();
  supersede();
  return promise;
}

function normalizeResets(resets: number[] | undefined, n: number): number[] {
  if (!resets || resets.length === 0) return [];
  const sorted = resets.filter((r) => Number.isInteger(r) && r > 0 && r < n).sort((a, b) => a - b);
  return sorted.filter((r, i) => i === 0 || r !== sorted[i - 1]);
}

/** 53-bit hash (cyrb53) of the lines, line boundaries included, and the resets. */
function hashDoc(lines: string[], resets: number[]): string {
  let h1 = 0xdeadbeef;
  let h2 = 0x41c6ce57;
  for (const line of lines) {
    for (let i = 0; i < line.length; i++) {
      const c = line.charCodeAt(i);
      h1 = Math.imul(h1 ^ c, 2654435761);
      h2 = Math.imul(h2 ^ c, 1597334677);
    }
    h1 = Math.imul(h1 ^ 10, 2654435761);
    h2 = Math.imul(h2 ^ 10, 1597334677);
  }
  for (const r of resets) {
    h1 = Math.imul(h1 ^ r, 2654435761);
    h2 = Math.imul(h2 ^ r, 1597334677);
  }
  h1 = Math.imul(h1 ^ (h1 >>> 16), 2246822507) ^ Math.imul(h2 ^ (h2 >>> 13), 3266489909);
  h2 = Math.imul(h2 ^ (h2 >>> 16), 2246822507) ^ Math.imul(h1 ^ (h1 >>> 13), 3266489909);
  return (4294967296 * (2097151 & h2) + (h1 >>> 0)).toString(36);
}

/** Posts to the worker, creating it on first use; false when no worker can run (highlighting is then off). */
function send(msg: ToWorker): boolean {
  if (!worker) {
    let w: Worker;
    try {
      w = new Worker(new URL('./highlight.worker.ts', import.meta.url), { type: 'module' });
    } catch (err) {
      console.warn('[highlight] worker unavailable', err);
      disable();
      return false;
    }
    w.onmessage = (e: MessageEvent<FromWorker>) => {
      if (worker === w) receive(e.data);
    };
    w.onerror = () => {
      if (worker === w) restart();
    };
    w.onmessageerror = () => {
      if (worker === w) restart();
    };
    worker = w;
  }
  worker.postMessage(msg);
  return true;
}

function receive(msg: FromWorker): void {
  if ('busy' in msg) busy = msg.busy;
  else {
    const req = pending.get(msg.id);
    if (req) {
      failures = 0;
      settle(req, store(req.key, req.job.lines, req.chars, msg.data));
    }
  }
  arm();
}

/** (Re)starts the watchdog while requests are pending; every message from the worker counts as progress. */
function arm(): void {
  clearTimeout(watchdog);
  watchdog = pending.size > 0 ? setTimeout(onTimeout, WATCHDOG_MS) : undefined;
}

function onTimeout(): void {
  watchdog = undefined;
  if (pending.size > 0) restart();
}

/**
 * Replaces a hung or crashed worker. The document it was working on is remembered as unhighlightable (even when that
 * request was superseded meanwhile) and its request resolves null; the other pending jobs never reached that point and
 * are re-sent to the new worker.
 */
function restart(): void {
  worker?.terminate();
  worker = null;
  const key = pending.get(busy)?.key ?? dropped.get(busy);
  busy = 0;
  dropped.clear();
  if (key !== undefined) {
    const entry = store(key, [], 0, null);
    const culprit = byKey.get(key);
    if (culprit) settle(culprit, entry);
  }
  if (++failures >= MAX_FAILURES) {
    disable();
    return;
  }
  for (const req of pending.values()) if (!send(req.job)) return;
  arm();
}

function disable(): void {
  disabled = true;
  worker?.terminate();
  worker = null;
  clearTimeout(watchdog);
  watchdog = undefined;
  for (const req of [...pending.values()]) settle(req, null);
}

/** Drops the oldest requests beyond MAX_LIVE (resolving them null) and tells the worker to forget them. */
function supersede(): void {
  for (const req of pending.values()) {
    if (pending.size <= MAX_LIVE) break;
    settle(req, null);
    dropped.set(req.job.id, req.key);
    const cancel: ToWorker = { cancel: req.job.id };
    worker?.postMessage(cancel);
  }
  for (const id of dropped.keys()) {
    if (dropped.size <= MAX_DROPPED) break;
    dropped.delete(id);
  }
}

function settle(req: Pending, entry: Entry | null): void {
  pending.delete(req.job.id);
  if (byKey.get(req.key) === req) byKey.delete(req.key);
  req.resolve(entry ? expand(entry) : null);
}

/** Caches a result as the newest entry and evicts the oldest beyond the limits. A null entry keeps no lines. */
function store(key: string, lines: string[], chars: number, data: PackedTokens | null): Entry {
  const old = cache.get(key);
  if (old) {
    cache.delete(key);
    cacheChars -= old.chars;
  }
  const entry: Entry = data ? { lines, data, chars, view: null } : { lines: [], data: null, chars: 0, view: null };
  cache.set(key, entry);
  cacheChars += entry.chars;
  for (const [k, e] of cache) {
    if (cache.size <= 1 || (cache.size <= CACHE_DOCS && cacheChars <= CACHE_CHARS)) break;
    cache.delete(k);
    cacheChars -= e.chars;
  }
  return entry;
}

/** Packed tokens → Token[][] (slices of the original lines); reused while a caller still holds the last result. */
function expand(entry: Entry): Token[][] | null {
  const { data, lines } = entry;
  if (!data) return null;
  const live = entry.view?.deref();
  if (live) return live;
  const { starts, ends, colors, palette } = data;
  const out = new Array<Token[]>(lines.length);
  for (let i = 0; i < lines.length; i++) {
    const line = lines[i];
    const first = starts[i];
    const row = new Array<Token>(starts[i + 1] - first);
    let pos = 0;
    for (let k = 0; k < row.length; k++) {
      const end = ends[first + k];
      row[k] = [line.slice(pos, end), palette[colors[first + k]] ?? ''];
      pos = end;
    }
    out[i] = row;
  }
  entry.view = new WeakRef(out);
  return out;
}
