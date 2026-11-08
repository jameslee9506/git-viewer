// Graph rows live in the backend; this store fetches them in fixed chunks on demand.
// Rows of the previous generation are kept as placeholders until fresh chunks arrive (no flicker on refresh).
import { api, errorMessage, isStale } from '../api';
import type { GraphOptions, GraphRow, GraphSummary, SearchQuery } from '../types';

export const CHUNK = 256;
/** Chunks kept per map (fresh / stale); the farthest from the viewed range are evicted first. */
const MAX_CHUNKS = 64;
const NO_ROWS: readonly number[] = [];

export interface SearchState {
  query: SearchQuery;
  /** Matching rows, ascending, of generation `gen`. */
  rows: number[];
  /** The graph generation `rows` index: in another generation the same rows hold other commits. */
  gen: number;
  /** Index into rows of the current match, -1 = none. */
  index: number;
  busy: boolean;
  error: string | null;
}

export class GraphStore {
  readonly repo: string;
  summary = $state.raw<GraphSummary | null>(null);
  /** Bumped whenever cached rows change. row() reads it, so templates re-render on arrival. */
  version = $state(0);
  loading = $state(false);
  error = $state<string | null>(null);
  /** Row index of the primary selection, null when not a loaded row. */
  selectedRow = $state<number | null>(null);
  /** Row index of the compare target (second selection), if any. */
  compareRow = $state<number | null>(null);
  /** Request for GraphView to scroll a row into view. seq distinguishes repeated requests. */
  scrollRequest = $state.raw<{ row: number; align: 'auto' | 'center'; seq: number } | null>(null);
  search = $state.raw<SearchState | null>(null);
  /** Matching rows, only while they index the current generation (reloads re-run the search). */
  matchRows = $derived.by((): readonly number[] => {
    const st = this.search;
    return st && st.gen === this.summary?.generation ? st.rows : NO_ROWS;
  });
  /** Set of matching rows for O(1) highlight checks. */
  matchSet = $derived(new Set(this.matchRows));
  /** Row of the current match in the current generation, or null. */
  currentMatch = $derived.by(() => {
    const st = this.search;
    return st && st.index >= 0 && st.gen === this.summary?.generation ? (st.rows[st.index] ?? null) : null;
  });
  /**
   * Called when the backend serves a graph generation this store never applied (a superseded load
   * took its generation after the latest one, or the latest one failed after it): every row, find and
   * search call for the applied generation then fails with 'stale'. Also when the latest load's walk
   * was cancelled by a load whose answer may never come here. The owner loads the graph again.
   * Called at most once per applied generation.
   */
  onOrphaned: (() => void) | null = null;

  #chunks = new Map<number, GraphRow[]>();
  #stale = new Map<number, GraphRow[]>();
  #pending = new Map<number, Promise<GraphRow[] | undefined>>();
  #seq = 0;
  #loadSeq = 0;
  #searchSeq = 0;
  /** The latest runSearch: a run superseded by a re-run of its query resolves with it. */
  #searchRun: Promise<void> = Promise.resolve();
  /** Chunk index at the center of the last ensure()d range (eviction keeps what is near it). */
  #center = 0;
  /** Highest generation any graph_load answered, superseded loads included. */
  #maxGen = -1;
  /** Generation onOrphaned was last called for. */
  #orphanedGen: number | null = null;

  constructor(repo: string) {
    this.repo = repo;
  }

  get generation(): number {
    return this.summary?.generation ?? -1;
  }

  get total(): number {
    return this.summary?.total ?? 0;
  }

  /**
   * (Re)load the graph. Returns null when superseded by a newer load or on error. A new generation
   * re-runs the active search (its matches index the old one and are hidden meanwhile). A load that
   * changes nothing gets the current generation back: rows and matches stay.
   */
  async load(opts: GraphOptions, wip: boolean): Promise<GraphSummary | null> {
    const seq = ++this.#loadSeq;
    this.loading = true;
    /** The backend cancelled this load's walk for a graph_load with a later ticket. */
    let cancelled = false;
    try {
      let summary = await this.#graphLoad(opts, wip);
      // A load asked for earlier took a later generation and replaced this one's graph: load again.
      if (seq === this.#loadSeq && summary.generation < this.#maxGen) summary = await this.#graphLoad(opts, wip);
      if (seq !== this.#loadSeq) {
        // Superseded. Once the latest load has ended, a newer generation than it applied orphans it.
        if (!this.loading && summary.generation > this.generation) this.#orphaned();
        return null;
      }
      this.#apply(summary);
      return summary;
    } catch (e) {
      if (seq === this.#loadSeq) {
        // Not a failure: the walk was superseded.
        if (errorMessage(e) === 'cancelled') cancelled = true;
        else this.error = errorMessage(e);
      }
      return null;
    } finally {
      if (seq === this.#loadSeq) {
        this.loading = false;
        // E.g. this load failed after a superseded one had installed a newer graph. A walk cancelled by a
        // load that is not this store's latest (an earlier one the backend took later, or a closed tab's
        // of this repo, whose answer never comes here) leaves nothing to apply either: load again.
        if (cancelled || this.#maxGen > this.generation) this.#orphaned();
      }
    }
  }

  async #graphLoad(opts: GraphOptions, wip: boolean): Promise<GraphSummary> {
    const summary = await api.graphLoad(this.repo, opts, wip);
    this.#maxGen = Math.max(this.#maxGen, summary.generation);
    return summary;
  }

  /** Make `summary` the current generation (nothing to do when it already is). */
  #apply(summary: GraphSummary): void {
    this.error = null;
    if (summary.generation === this.summary?.generation) return;
    if (this.#chunks.size) this.#stale = this.#chunks;
    this.#chunks = new Map();
    this.#evict(this.#stale);
    this.#pending = new Map();
    this.summary = summary;
    // Re-run against the new generation. Its busy state replaces the old matches in this same update,
    // so no frame shows them at shifted rows; a run still waiting for the old generation's answer
    // resolves with the re-run.
    if (this.search) void this.runSearch(this.search.query);
    this.version++;
  }

  /**
   * Note a row / find / search call for generation `gen` that failed with `e`: 'stale' for the applied
   * generation while no load runs means the backend dropped it (see onOrphaned).
   */
  checkStale(e: unknown, gen: number): void {
    if (isStale(e) && gen === this.generation) this.#orphaned();
  }

  #orphaned(): void {
    const gen = this.generation;
    if (this.loading || this.#orphanedGen === gen) return;
    this.#orphanedGen = gen;
    this.onOrphaned?.();
  }

  /** Cached row (fresh, else previous-generation placeholder). Reactive via `version`. */
  row(i: number): GraphRow | undefined {
    void this.version;
    if (i < 0 || i >= this.total) return undefined;
    const c = Math.floor(i / CHUNK);
    const chunk = this.#chunks.get(c) ?? this.#stale.get(c);
    return chunk?.[i - c * CHUNK];
  }

  /** True when row i belongs to the current generation. */
  isFresh(i: number): boolean {
    void this.version;
    return this.#chunks.has(Math.floor(i / CHUNK));
  }

  /** Make sure rows [start, end) are (being) fetched. */
  ensure(start: number, end: number): void {
    const s = this.summary;
    if (!s || s.total === 0) return;
    const lo = Math.max(0, start);
    const hi = Math.min(end, s.total);
    if (hi <= lo) return;
    const first = Math.floor(lo / CHUNK);
    const last = Math.floor((hi - 1) / CHUNK);
    this.#center = Math.floor((first + last) / 2);
    for (let c = first; c <= last; c++) {
      if (!this.#chunks.has(c) && !this.#pending.has(c)) void this.#fetch(c, s.generation);
    }
  }

  /** Fetch chunk c of generation gen into the cache; resolves with its rows (undefined if dropped). */
  #fetch(c: number, gen: number): Promise<GraphRow[] | undefined> {
    const pending = this.#pending;
    const p: Promise<GraphRow[] | undefined> = api
      .graphRows(this.repo, gen, c * CHUNK, (c + 1) * CHUNK)
      .then((rows) => {
        if (this.summary?.generation !== gen) return undefined;
        this.#chunks.set(c, rows);
        this.#stale.delete(c);
        // Never this chunk: far from the viewed range, it is what a far jump is about to read.
        this.#evict(this.#chunks, c);
        this.version++;
        return rows;
      })
      .catch((e: unknown) => {
        if (!isStale(e)) this.error = errorMessage(e);
        this.checkStale(e, gen);
        return undefined;
      })
      .finally(() => {
        if (pending.get(c) === p) pending.delete(c);
      });
    pending.set(c, p);
    return p;
  }

  /** Drop the chunks farthest from the viewed range (except `keep`) once a map exceeds MAX_CHUNKS. */
  #evict(map: Map<number, GraphRow[]>, keep?: number): void {
    if (map.size <= MAX_CHUNKS) return;
    const center = this.#center;
    const far = [...map.keys()]
      .filter((c) => c !== keep)
      .sort((a, b) => Math.abs(b - center) - Math.abs(a - center));
    for (const c of far.slice(0, map.size - MAX_CHUNKS)) map.delete(c);
  }

  /** Row i of the current generation, fetching its chunk if needed. */
  async getRow(i: number): Promise<GraphRow | undefined> {
    const s = this.summary;
    if (!s || i < 0 || i >= s.total) return undefined;
    const c = Math.floor(i / CHUNK);
    // The fetched rows themselves: a chunk far from the viewed range may be evicted again by the time
    // this resumes (another chunk landing meanwhile).
    const rows = this.#chunks.get(c) ?? (await (this.#pending.get(c) ?? this.#fetch(c, s.generation)));
    if (this.summary?.generation !== s.generation) return undefined;
    return rows?.[i - c * CHUNK];
  }

  /** Row index of a full oid ('' = WIP row) in the current generation, or null. */
  async find(oid: string): Promise<number | null> {
    for (;;) {
      const s = this.summary;
      if (!s) return null;
      if (oid === '') return s.wipRow;
      for (const [c, rows] of this.#chunks) {
        const k = rows.findIndex((r) => r.oid === oid);
        if (k >= 0) return c * CHUNK + k;
      }
      try {
        const row = await api.graphFind(this.repo, s.generation, oid);
        if (this.summary?.generation === s.generation) return row;
      } catch (e) {
        if (this.summary?.generation === s.generation) {
          this.checkStale(e, s.generation);
          return null;
        }
      }
      // A newer generation was applied meanwhile (its rows may be shifted, and the backend may have
      // answered 'stale' for the old one): look the oid up in that one.
    }
  }

  scrollTo(row: number, align: 'auto' | 'center' = 'auto'): void {
    this.scrollRequest = { row, align, seq: ++this.#seq };
  }

  /** Search the current generation. Resolves once the matches of `query` are in (or it was dropped). */
  runSearch(query: SearchQuery): Promise<void> {
    const s = this.summary;
    const text = query.text.trim();
    if (!s || !text) {
      this.clearSearch();
      return Promise.resolve();
    }
    const seq = ++this.#searchSeq;
    const gen = s.generation;
    // While it runs, this generation's matches stay (no flicker while typing; a re-run keeps its place).
    const prev = this.search?.gen === gen ? this.search : null;
    this.search = { query, rows: prev?.rows ?? [], gen, index: prev?.index ?? -1, busy: true, error: null };
    const run = this.#search(seq, gen, query, text);
    this.#searchRun = run;
    return run;
  }

  async #search(seq: number, gen: number, query: SearchQuery, text: string): Promise<void> {
    try {
      const rows = await api.graphSearch(this.repo, gen, { ...query, text });
      // A new generation re-runs the search (see #apply), so this is superseded then too.
      if (seq !== this.#searchSeq) return this.#rerunOf(query);
      // Start at the first match at/after the selection.
      const sel = this.selectedRow ?? -1;
      let index = rows.findIndex((r) => r >= sel);
      if (index < 0) index = rows.length ? 0 : -1;
      this.search = { query, rows, gen, index, busy: false, error: null };
    } catch (e) {
      if (seq !== this.#searchSeq) return this.#rerunOf(query);
      this.checkStale(e, gen);
      this.search = { query, rows: [], gen, index: -1, busy: false, error: isStale(e) ? null : errorMessage(e) };
    }
  }

  /** The run that superseded a run of `query`, when it re-runs that same query (after a reload). */
  #rerunOf(query: SearchQuery): Promise<void> | undefined {
    return this.search?.query === query ? this.#searchRun : undefined;
  }

  /** Step to the next/previous match; returns its row index. */
  stepMatch(dir: 1 | -1): number | null {
    const st = this.search;
    if (!st || st.gen !== this.generation || st.rows.length === 0) return null;
    const index = (st.index + dir + st.rows.length) % st.rows.length;
    this.search = { ...st, index };
    return st.rows[index];
  }

  clearSearch(): void {
    this.#searchSeq++;
    this.search = null;
  }
}
