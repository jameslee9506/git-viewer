// Promise caches for immutable backend results (keyed by oids); working-tree targets are never cached. Each cache is
// an LRU bounded by entry count and by the approximate WebView heap its results take. A result too big to keep (over
// a quarter of its cache's budget) or a forced unlimited diff is only held weakly: going back to it stays instant
// while something still shows it, and it is freed once nothing does.
import { api } from '../api';
import type { CommitDetails, DiffRequest, DiffTarget, FileChange, FileDiff } from '../types';
import { targetKey } from './repo.svelte';

/**
 * Least-recently-used map bounded by entry count and, optionally, by total weight (e.g. approximate bytes). A weight
 * can be set after insertion (a promise is weighed once it resolves). A value heavier than a quarter of the budget is
 * not kept, so one huge value never flushes everything else.
 */
export class LRU<K, V> {
  #map = new Map<K, { v: V; w: number }>();
  #max: number;
  #budget: number;
  #total = 0;

  constructor(max: number, budget = Infinity) {
    this.#max = max;
    this.#budget = budget;
  }

  get(k: K): V | undefined {
    const e = this.#map.get(k);
    if (!e) return undefined;
    this.#map.delete(k);
    this.#map.set(k, e);
    return e.v;
  }

  /** Store as the newest entry, then evict the oldest ones beyond the limits. */
  set(k: K, v: V, weight = 0): void {
    this.delete(k);
    this.#map.set(k, { v, w: 0 });
    this.weigh(k, v, weight);
  }

  /**
   * Set the weight of `k` while it still holds `v`, then evict the oldest entries beyond the limits. False when `v`
   * is not kept: no longer cached, too heavy (then removed) or evicted to make room.
   */
  weigh(k: K, v: V, weight: number): boolean {
    const e = this.#map.get(k);
    if (!e || e.v !== v) return false;
    if (weight > this.#budget / 4) {
      this.delete(k);
      return false;
    }
    this.#total += weight - e.w;
    e.w = weight;
    this.#trim();
    return this.#map.get(k) === e;
  }

  /** Remove `k` (when `v` is given: only while `k` still holds it). */
  delete(k: K, v?: V): void {
    const e = this.#map.get(k);
    if (!e || (v !== undefined && e.v !== v)) return;
    this.#map.delete(k);
    this.#total -= e.w;
  }

  /** Remove every entry whose key matches. */
  deleteWhere(match: (k: K) => boolean): void {
    for (const k of this.#map.keys()) if (match(k)) this.delete(k);
  }

  clear(): void {
    this.#map.clear();
    this.#total = 0;
  }

  /** Oldest first: evict while over the count, or over the budget (weightless entries, e.g. loads, don't help). */
  #trim(): void {
    for (const [k, e] of this.#map) {
      const crowded = this.#map.size > this.#max;
      if (!crowded && this.#total <= this.#budget) return;
      if (crowded || e.w > 0) {
        this.#map.delete(k);
        this.#total -= e.w;
      }
    }
  }
}

/** Targets whose results can never change (everything addressed by oid). */
export function isImmutableTarget(t: DiffTarget): boolean {
  return t.kind === 'commit' || t.kind === 'range' || t.kind === 'stash' || t.kind === 'stashUntracked';
}

/** One kind of result: the LRU, plus weak references to the results it does not keep. */
class ResultCache<T extends object> {
  #lru: LRU<string, Promise<T>>;
  #weigh: (v: T) => number;
  #weak = new LRU<string, WeakRef<T>>(16);

  constructor(max: number, budget: number, weigh: (v: T) => number) {
    this.#lru = new LRU<string, Promise<T>>(max, budget);
    this.#weigh = weigh;
  }

  /** The result for `key`, loaded on a miss. `keep` false: huge by definition, so only ever held weakly. */
  get(key: string, load: () => Promise<T>, keep = true): Promise<T> {
    const hit = this.#lru.get(key);
    if (hit) return hit;
    const held = this.#weak.get(key)?.deref();
    if (held) return Promise.resolve(held);
    const p = load();
    // Weightless while in flight: concurrent requests share the load.
    this.#lru.set(key, p);
    p.then(
      (v) => {
        if (keep && this.#lru.weigh(key, p, this.#weigh(v))) return;
        this.#lru.delete(key, p);
        this.#weak.set(key, new WeakRef(v));
      },
      () => this.#lru.delete(key, p),
    );
    return p;
  }

  /** Drop every result whose key matches. */
  forget(match: (key: string) => boolean): void {
    this.#lru.deleteWhere(match);
    this.#weak.deleteWhere(match);
  }
}

// Approximate WebView heap of decoded results: a fixed cost per object or line plus the text it carries.
const MB = 1024 * 1024;
/** The fixed part of one result. */
const BASE_BYTES = 1024;
/** One diff line (a 4-element array and its string), hunk header or extended header line. */
const LINE_BYTES = 150;
/** One FileChange (a 10-field object and its strings). */
const FILE_BYTES = 250;

function filesBytes(files: FileChange[]): number {
  let n = BASE_BYTES;
  for (const f of files) n += FILE_BYTES + f.path.length + (f.oldPath?.length ?? 0);
  return n;
}

function detailsBytes(d: CommitDetails): number {
  return filesBytes(d.files) + d.subject.length + d.body.length;
}

function diffBytes(d: FileDiff): number {
  let n = BASE_BYTES;
  for (const l of d.headerLines) n += LINE_BYTES + l.length;
  for (const h of d.hunks) {
    n += LINE_BYTES + h.header.length;
    for (const l of h.lines) n += LINE_BYTES + l[3].length;
  }
  return n;
}

const detailsCache = new ResultCache<CommitDetails>(400, 32 * MB, detailsBytes);
const filesCache = new ResultCache<FileChange[]>(200, 16 * MB, filesBytes);
const diffCache = new ResultCache<FileDiff>(150, 64 * MB, diffBytes);

export function getCommitDetails(repo: string, oid: string): Promise<CommitDetails> {
  return detailsCache.get(`${repo}\0${oid}`, () => api.commitDetails(repo, oid));
}

export function getDiffFiles(repo: string, target: DiffTarget): Promise<FileChange[]> {
  if (!isImmutableTarget(target)) return api.diffFiles(repo, target);
  return filesCache.get(`${repo}\0${targetKey(target)}`, () => api.diffFiles(repo, target));
}

export function getFileDiff(repo: string, req: DiffRequest): Promise<FileDiff> {
  if (!isImmutableTarget(req.target)) return api.fileDiff(repo, req);
  const key = [repo, targetKey(req.target), req.path, req.oldPath ?? '', req.context ?? '', req.ignoreWhitespace ? 1 : 0, req.maxBytes ?? ''].join('\0');
  // `maxBytes: 0` (the Load button) fetches a diff over the default limit: never kept, only held weakly.
  return diffCache.get(key, () => api.fileDiff(repo, req), req.maxBytes !== 0);
}

/** Drop everything cached for `repo` (call when its tab closes). */
export function forgetRepo(repo: string): void {
  const prefix = `${repo}\0`;
  const mine = (key: string) => key.startsWith(prefix);
  detailsCache.forget(mine);
  filesCache.forget(mine);
  diffCache.forget(mine);
}
