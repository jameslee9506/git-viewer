// Per-tab repository store: refs, working status, graph, selection and the center view.
import { getContext, setContext } from 'svelte';
import { api, errorMessage, onRepoChanged } from '../api';
import type {
  BranchTrack,
  ChangeKind,
  ChangeStatus,
  DiffTarget,
  GraphOptions,
  GraphRow,
  RefsSnapshot,
  RepoInfo,
  RepoOperation,
  StatusEntry,
  WorkingStatus,
} from '../types';
import { AutoFetch } from './autofetch.svelte';
import { CHUNK, GraphStore } from './graph.svelte';
import { WriteOps } from './ops.svelte';
import { loadJSON, saveJSON } from './persist';
import { settings } from './settings.svelte';

export type Selection =
  | { kind: 'none' }
  | { kind: 'commit'; oid: string }
  /** Working tree of a worktree directory (the opened one or another linked worktree). */
  | { kind: 'wip'; cwd: string }
  | { kind: 'stash'; oid: string }
  /** from = older, to = newer; to '' = the opened worktree's working tree (its WIP row). */
  | { kind: 'compare'; from: string; to: string; range?: CommitRange };

/**
 * A ⇧-range of commits: `first` (the oldest, included) up to the compare's `to`. When `to` reaches
 * `first` along first parents, the `count` commits of that chain are combined (`from` = first's parent,
 * or the empty tree for a root commit); otherwise `count` is null and the two ends are compared
 * (`from` = `first`). `anchor` is the end that stays put while ⇧-click / ⇧↑↓ move the other one.
 */
export interface CommitRange {
  first: string;
  count: number | null;
  anchor: 'first' | 'to';
}

/** The empty tree of the object format of `oid` (SHA-1: 40 hex digits, SHA-256: 64). */
function emptyTree(oid: string): string {
  return oid.length === 64
    ? '6ef19b41225c5369f1c104d45d8d85efa9b057b53b14b4b9b939dd74decc5321'
    : '4b825dc642cb6eb9a060e54bf8d69288fbee4904';
}

/** Field-by-field equality of two statuses, without allocating (cheap next to the IPC that brought them). */
function sameStatus(a: WorkingStatus, b: WorkingStatus): boolean {
  return (
    a.cwd === b.cwd &&
    a.head.oid === b.head.oid &&
    a.head.branch === b.head.branch &&
    a.head.detached === b.head.detached &&
    a.head.unborn === b.head.unborn &&
    a.upstream === b.upstream &&
    a.ahead === b.ahead &&
    a.behind === b.behind &&
    sameOperation(a.operation, b.operation) &&
    // The list that can be huge (untracked) last.
    sameEntries(a.conflicted, b.conflicted) &&
    sameEntries(a.staged, b.staged) &&
    sameEntries(a.unstaged, b.unstaged) &&
    sameEntries(a.untracked, b.untracked)
  );
}

function sameOperation(a: RepoOperation | null, b: RepoOperation | null): boolean {
  if (!a || !b) return a === b;
  return a.kind === b.kind && a.step === b.step && a.total === b.total && a.detail === b.detail;
}

function sameEntries(a: StatusEntry[], b: StatusEntry[]): boolean {
  if (a.length !== b.length) return false;
  for (let i = 0; i < a.length; i++) {
    const x = a[i];
    const y = b[i];
    if (
      x.path !== y.path ||
      x.oldPath !== y.oldPath ||
      x.status !== y.status ||
      x.conflict !== y.conflict ||
      x.submodule !== y.submodule
    ) {
      return false;
    }
  }
  return true;
}

/** A promise with its resolve function (lib is ES2022: no Promise.withResolvers). */
interface Deferred {
  promise: Promise<void>;
  resolve: () => void;
}

function deferred(): Deferred {
  let resolve!: () => void;
  const promise = new Promise<void>((r) => {
    resolve = r;
  });
  return { promise, resolve };
}

/**
 * Row reader for a scan that reads rows in order, all from graph generation `gen`: cached rows of
 * that generation, else whole chunks fetched once each, bypassing the chunk cache (which may evict a
 * chunk far from the viewed range as soon as it lands). Resolves null for a row that cannot be read
 * (out of range, failed fetch, newer generation).
 */
export function rowReader(g: GraphStore, gen: number): (i: number) => Promise<GraphRow | null> {
  let chunk = -1;
  let rows: Promise<GraphRow[] | null> = Promise.resolve(null);
  return async (i) => {
    if (g.generation !== gen || i < 0 || i >= g.total) return null;
    if (g.isFresh(i)) return g.row(i) ?? null;
    const c = Math.floor(i / CHUNK);
    if (c !== chunk) {
      chunk = c;
      rows = api.graphRows(g.repo, gen, c * CHUNK, (c + 1) * CHUNK).catch((e: unknown) => {
        g.checkStale(e, gen);
        return null;
      });
    }
    return (await rows)?.[i - c * CHUNK] ?? null;
  };
}

/** Minimal file identity used for diff navigation. */
export interface FileRef {
  path: string;
  oldPath: string | null;
  status: ChangeStatus;
}

/** A file inside a diff list: which target to diff it against. */
export interface DiffItem {
  target: DiffTarget;
  file: FileRef;
}

/** Canonical identity of a diff target (independent of object key order). */
export function targetKey(t: DiffTarget): string {
  switch (t.kind) {
    case 'commit':
      return `commit:${t.oid}:${t.parent ?? ''}`;
    case 'range':
      return `range:${t.from}:${t.to}`;
    case 'staged':
    case 'unstaged':
    case 'untracked':
    case 'conflicted':
      return `${t.kind}:${t.cwd}`;
    case 'worktree':
      return `worktree:${t.cwd}:${t.oid}`;
    case 'stash':
    case 'stashUntracked':
      return `${t.kind}:${t.oid}`;
  }
}

/** Stable identity of a diff item (target + path), e.g. to highlight the open file in a list. */
export function diffItemKey(item: DiffItem): string {
  return `${targetKey(item.target)}\0${item.file.path}`;
}

/** Key of the diff item currently shown in the center, or null. */
export function activeDiffKey(view: CenterView): string | null {
  return view.kind === 'diff' ? diffItemKey(view.items[view.index]) : null;
}

export type CenterView =
  | { kind: 'graph' }
  /** items = sibling list for prev/next navigation; index = current. */
  | { kind: 'diff'; items: DiffItem[]; index: number }
  /** rev null = working tree; cwd = worktree directory for working-tree blame. */
  | { kind: 'blame'; path: string; rev: string | null; cwd: string | null }
  | { kind: 'history'; path: string; rev: string | null }
  | { kind: 'tree'; rev: string; path: string | null };

interface RepoPrefs {
  hiddenRefs: string[];
  onlyRefs: string[];
}

/** The inline name input open in the graph: at row `row`, for a new branch or tag on the commit `oid`. */
export interface RefPrompt {
  kind: 'branch' | 'tag';
  row: number;
  oid: string;
  /** A branch: switch to it once created (the toolbar's Branch, at the HEAD commit). */
  checkout: boolean;
}

const KEY = Symbol('repo');

export function setRepoContext(repo: RepoStore): void {
  setContext(KEY, repo);
}

export function getRepo(): RepoStore {
  return getContext<RepoStore>(KEY);
}

export class RepoStore {
  readonly id: string;
  info = $state.raw<RepoInfo>(null!);
  refs = $state.raw<RefsSnapshot | null>(null);
  status = $state.raw<WorkingStatus | null>(null);
  error = $state<string | null>(null);
  ready = $state(false);
  selection = $state.raw<Selection>({ kind: 'none' });
  view = $state.raw<CenterView>({ kind: 'graph' });
  hiddenRefs = $state.raw<string[]>([]);
  onlyRefs = $state.raw<string[]>([]);
  /** Incremented to ask the graph search box to open + focus. */
  findRequest = $state(0);
  /**
   * Incremented whenever the content of a worktree may have changed: working-tree diffs, and the WIP
   * of other worktrees (DetailsPanel), refetch on change.
   */
  worktreeVersion = $state(0);
  /** The branch / tag name input open in the graph (GraphView renders it in that row), or null. */
  refPrompt = $state.raw<RefPrompt | null>(null);
  readonly graph: GraphStore;
  /** Write operations (toolbar, staging, commit), one after the other per repo. */
  readonly ops: WriteOps;
  /** The background fetch of the repository's remotes, shared with the tabs of its other worktrees. */
  readonly autoFetch: AutoFetch;

  dirty = $derived.by(() => {
    const s = this.status;
    return !!s && s.staged.length + s.unstaged.length + s.untracked.length + s.conflicted.length > 0;
  });
  /**
   * The graph has a WIP row: the tree has changes, or an operation (merge, rebase, cherry-pick, …) is
   * in progress. The WIP panel is where an operation shows and is continued or aborted, also once its
   * conflicts are resolved to nothing and the tree is clean again.
   */
  wipShown = $derived(this.dirty || !!this.status?.operation);

  #unlisten: (() => void) | null = null;
  #disposed = false;

  /** The tab closed: nothing is loaded any more, and what its operation queue still holds must not reach a remote. */
  get disposed(): boolean {
    return this.#disposed;
  }
  /** Watcher kinds collected for the next refresh request (see #queue). */
  #queuedKinds = new Set<ChangeKind>();
  #timer: ReturnType<typeof setTimeout> | undefined;
  /** What the next refresh pass loads (requests merged), and the promise their callers wait on. */
  #pending: Set<ChangeKind> | 'all' | null = null;
  #pendingDone: Deferred | null = null;
  /** A refresh pass is running (see #drain). */
  #draining = false;
  /** Request counters: a response older than the last one applied is dropped. */
  #refsSeq = 0;
  #statusSeq = 0;
  /** Last `repo_track` answer by full branch name: the counts `refs.locals` carry (see #withTrack). */
  #track = new Map<string, BranchTrack>();
  /** A track request is running / another was asked for meanwhile (see #loadTrack). */
  #trackBusy = false;
  #trackAgain = false;

  constructor(info: RepoInfo) {
    this.id = info.path;
    this.info = info;
    this.graph = new GraphStore(info.path);
    this.ops = new WriteOps(this);
    this.autoFetch = AutoFetch.join(this);
    this.graph.onOrphaned = () => {
      if (!this.#disposed) void this.reloadGraph();
    };
    const prefs = loadJSON<RepoPrefs>(`repo:${info.path}`, { hiddenRefs: [], onlyRefs: [] });
    this.hiddenRefs = prefs.hiddenRefs ?? [];
    this.onlyRefs = prefs.onlyRefs ?? [];
  }

  async init(): Promise<void> {
    const un = await onRepoChanged((e) => {
      if (e.repo === this.id) this.#queue(e.kinds);
    });
    if (this.#disposed) {
      un();
      return;
    }
    this.#unlisten = un;
    // The first load is the first refresh pass: watcher refreshes queue behind it instead of racing it
    // (an older status landing last would leave the graph's WIP row wrong). This resolves after this
    // pass only, not after the ones queued behind it.
    await this.refresh('all');
    this.ready = true;
  }

  dispose(): void {
    this.#disposed = true;
    this.graph.onOrphaned = null;
    if (this.#timer) clearTimeout(this.#timer);
    this.#unlisten?.();
    this.#unlisten = null;
    this.autoFetch.leave(this);
    void api.closeRepo(this.id).catch(() => {});
  }

  graphOptions(): GraphOptions {
    return {
      order: settings.graphOrder,
      limit: settings.maxCommits,
      includeRemotes: settings.showRemotes,
      includeTags: settings.showTags,
      includeStashes: settings.showStashes,
      hiddenRefs: this.hiddenRefs,
      onlyRefs: this.onlyRefs,
      pinHeadLane: settings.pinHeadLane,
    };
  }

  async loadRefs(): Promise<void> {
    const seq = ++this.#refsSeq;
    try {
      const refs = await api.refs(this.id);
      if (seq === this.#refsSeq) this.refs = this.#withTrack(refs);
    } catch (e) {
      if (seq === this.#refsSeq) this.error = errorMessage(e);
    }
  }

  /**
   * `refs` with each local branch's ahead/behind from the last track answer (a snapshot has none):
   * a new snapshot shows the previous counts until the next answer lands. `refs` itself when no count
   * changes; otherwise a copy that keeps every other part (so e.g. its worktree list stays the same).
   */
  #withTrack(refs: RefsSnapshot): RefsSnapshot {
    let changed = false;
    const locals = refs.locals.map((b) => {
      const t = b.upstream != null && !b.upstreamGone ? this.#track.get(b.fullName) : undefined;
      const ahead = t?.ahead ?? 0;
      const behind = t?.behind ?? 0;
      if (b.ahead === ahead && b.behind === behind) return b;
      changed = true;
      return { ...b, ahead, behind };
    });
    return changed ? { ...refs, locals } : refs;
  }

  /**
   * Load the local branches' ahead/behind; not awaited by a refresh (the walks behind them can take
   * seconds on a big history, and nothing else needs them). One request at a time: those made while it
   * runs are merged into one more run after it, so refreshes in a burst never pile up walks.
   */
  #loadTrack(): void {
    if (this.#disposed) return;
    if (this.#trackBusy) {
      this.#trackAgain = true;
      return;
    }
    this.#trackBusy = true;
    void (async () => {
      try {
        do {
          this.#trackAgain = false;
          let list: BranchTrack[];
          try {
            list = await api.track(this.id, [...this.#track.values()]);
          } catch (e) {
            // The counts are an extra: keep the last ones.
            console.warn('[repo] ahead/behind:', errorMessage(e));
            continue;
          }
          if (this.#disposed) return;
          this.#track = new Map(list.map((t) => [t.name, t]));
          const refs = this.refs;
          const next = refs && this.#withTrack(refs);
          if (next !== refs) this.refs = next;
        } while (this.#trackAgain && !this.#disposed);
      } finally {
        this.#trackBusy = false;
      }
    })();
  }

  async loadStatus(): Promise<void> {
    const seq = ++this.#statusSeq;
    if (this.info.isBare) {
      this.status = null;
      return;
    }
    try {
      const status = await api.status(this.id);
      if (seq !== this.#statusSeq) return;
      // An unchanged status (the usual case while files churn) keeps its object, so nothing derived
      // from it (file lists, counts, the graph's WIP markers) runs again.
      if (!this.status || !sameStatus(this.status, status)) this.status = status;
    } catch (e) {
      if (seq === this.#statusSeq) this.error = errorMessage(e);
    }
  }

  /** Reload the graph (a new generation re-runs the active search itself) and re-locate the selection. */
  async reloadGraph(): Promise<void> {
    // A closed tab's pending pass must not load into a reopened tab's backend handle (same repo id).
    if (this.#disposed) return;
    const s = await this.graph.load(this.graphOptions(), this.wipShown);
    if (s) await this.#resyncSelection();
  }

  /** The first graph load selected WIP / HEAD (done by whichever load installs first). */
  #initialSelectionDone = false;

  /**
   * Re-locate the selected commit(s) after the graph changed. It takes no #selSeq of its own (it must
   * not cancel a selection step in progress), so after a lookup it also checks that the selection is
   * still `sel`: a step that started before it may have landed meanwhile, with its own rows.
   */
  async #resyncSelection(): Promise<void> {
    const seq = this.#selSeq;
    const sel = this.selection;
    const g = this.graph;
    const gen = g.generation;
    // A newer selection, or a newer generation (whose own reload runs its own resync), wins.
    const moved = () => seq !== this.#selSeq || this.selection !== sel || g.generation !== gen;
    if (sel.kind === 'none') {
      g.selectedRow = null;
      if (this.#initialSelectionDone) return;
      const s = g.summary;
      const row = this.wipShown ? s?.wipRow : s?.headRow;
      if (row != null) {
        this.#initialSelectionDone = true;
        await this.selectRow(row, 'center');
        // Its row could not be read (e.g. the backend had dropped this generation) and nothing else was
        // selected since (selectRow took seq + 1): the next load selects again.
        if (this.selection.kind === 'none' && this.#selSeq === seq + 1) this.#initialSelectionDone = false;
      }
      return;
    }
    this.#initialSelectionDone = true;
    if (sel.kind === 'commit' || sel.kind === 'stash') {
      const row = await g.find(sel.oid);
      if (moved()) return;
      g.selectedRow = row;
      g.compareRow = null;
    } else if (sel.kind === 'wip') {
      const wip = sel.cwd === this.id ? (g.summary?.wipRow ?? null) : null;
      g.compareRow = null;
      if (wip != null) {
        // Also links up this worktree selected while clean (sidebar), now that it has a WIP row.
        g.selectedRow = wip;
        this.#wipLinked = true;
      } else if (this.#wipLinked) {
        // The selected WIP row is gone (the tree became clean, an operation ended): fall back to HEAD.
        const head = g.summary?.headRow;
        if (head != null) await this.selectRow(head);
        else g.selectedRow = null;
      } else {
        // Another worktree, or this one selected while clean: it stays selected, without a row.
        g.selectedRow = null;
      }
    } else if (sel.kind === 'compare') {
      // A range's compare row is its oldest commit (`from` may be that commit's parent).
      const [to, from] = await Promise.all([g.find(sel.to), g.find(sel.range?.first ?? sel.from)]);
      if (moved()) return;
      if (to == null && sel.to === '') {
        // The working tree became clean (no WIP row): fall back to HEAD, as a WIP selection does.
        const head = g.summary?.headRow;
        if (head != null) await this.selectRow(head);
        else await this.reveal(sel.from);
        return;
      }
      g.selectedRow = to;
      g.compareRow = from;
    } else {
      g.selectedRow = null;
    }
  }

  /**
   * Collect watcher kinds for 120 ms from the first event, then request one refresh for all of them.
   * The window is not extended by later events, so steady churn cannot postpone the refresh.
   */
  #queue(kinds: ChangeKind[]): void {
    for (const k of kinds) this.#queuedKinds.add(k);
    if (this.#timer) return;
    this.#timer = setTimeout(() => {
      this.#timer = undefined;
      const ks = [...this.#queuedKinds];
      this.#queuedKinds.clear();
      void this.refresh(ks);
    }, 120);
  }

  /**
   * Reload what `kinds` may have changed ('all': everything; first load, ⌘R, window focus). One pass
   * runs at a time; the requests made while it runs are merged into a single next pass, so a slow pass
   * under steady churn never builds a backlog (research watch-perf §1.6). Resolves once a pass that
   * started after this call has run. Never rejects.
   */
  refresh(kinds: ChangeKind[] | 'all' = 'all'): Promise<void> {
    if (this.#disposed) return Promise.resolve();
    const p = this.#pending;
    this.#pending = kinds === 'all' || p === 'all' ? 'all' : new Set([...(p ?? []), ...kinds]);
    const done = (this.#pendingDone ??= deferred());
    if (!this.#draining) void this.#drain();
    return done.promise;
  }

  /** Run refresh passes until no request is left (or the store is disposed). */
  async #drain(): Promise<void> {
    // Set before the first await: a refresh() made while a pass runs only merges its kinds.
    this.#draining = true;
    try {
      while (this.#pending && !this.#disposed) {
        const kinds = this.#pending;
        const done = this.#pendingDone;
        this.#pending = null;
        this.#pendingDone = null;
        try {
          await this.#pass(kinds);
        } catch (e) {
          this.error = errorMessage(e);
        } finally {
          done?.resolve();
        }
      }
    } finally {
      this.#draining = false;
      // Disposed with requests left: nothing will run them, settle their callers.
      this.#pending = null;
      this.#pendingDone?.resolve();
      this.#pendingDone = null;
    }
  }

  /** One refresh pass over `kinds`. */
  async #pass(kinds: Set<ChangeKind> | 'all'): Promise<void> {
    const has = (k: ChangeKind) => kinds === 'all' || kinds.has(k);
    // The working tree's files, its index, HEAD, an operation in progress, stashes.
    const content = has('worktree') || has('index') || has('head') || has('state') || has('stash');
    // Ref tips and config: they also change the status's upstream and ahead/behind (`git fetch`,
    // `git push`, `git branch -u` touch nothing else the watcher sees).
    const tracking = has('refs') || has('config');
    const needRefs = tracking || has('head') || has('stash');
    const hadWip = this.wipShown;
    const head = this.status?.head.oid;
    await Promise.all([needRefs ? this.loadRefs() : null, content || tracking ? this.loadStatus() : null]);
    // Refs and config leave this worktree's files alone, but can move its HEAD (its branch updated
    // from another worktree) and other worktrees' HEAD and ahead/behind, which their WIP shows.
    const headMoved = this.status?.head.oid !== head;
    const others = this.refs?.worktrees.some((w) => !w.isCurrent && !w.bare) ?? false;
    if (content || (tracking && (headMoved || others))) this.worktreeVersion++;
    try {
      if (needRefs || hadWip !== this.wipShown) await this.reloadGraph();
    } finally {
      // Once the graph is in (the first load's too): it must not wait for these walks.
      if (needRefs) this.#loadTrack();
    }
  }

  // ───────────── selection ─────────────

  /** Bumped by every selection change; async selection steps drop their result when superseded. */
  #selSeq = 0;

  /**
   * A WIP selection of the opened worktree made through its graph row (selectRow, or linked up by a
   * reload). It follows the row and falls back to HEAD when the row goes (the tree became clean); one
   * made otherwise (the sidebar's worktree row, blame's uncommitted lines) stays while clean.
   */
  #wipLinked = false;

  /**
   * Set the selection; the caller places the graph rows. A WIP selection made here starts without a
   * row (selectRow selects the WIP row; a reload links it up once there is one), so the previous
   * selection's rows are cleared: ⌘-click compares and arrow keys would take a stale one for its row.
   */
  select(sel: Selection): void {
    this.#selSeq++;
    this.#wipLinked = false;
    if (sel.kind === 'wip') {
      this.graph.selectedRow = null;
      this.graph.compareRow = null;
    }
    this.selection = sel;
  }

  /** Select the graph row i (commit / wip / stash) and scroll it into view. */
  async selectRow(i: number, align: 'auto' | 'center' = 'auto'): Promise<void> {
    const seq = ++this.#selSeq;
    const row = await this.graph.getRow(i);
    if (!row || seq !== this.#selSeq) return;
    this.graph.selectedRow = i;
    this.graph.compareRow = null;
    this.#wipLinked = row.kind === 'wip';
    if (row.kind === 'wip') this.selection = { kind: 'wip', cwd: this.id };
    else if (row.kind === 'stash') this.selection = { kind: 'stash', oid: row.oid };
    else this.selection = { kind: 'commit', oid: row.oid };
    this.graph.scrollTo(i, align);
  }

  /**
   * Show the working tree of the worktree at `cwd`: the opened one through its WIP row (scrolled into
   * view), another one — or the opened one while it has no row — by itself. Nothing when it shows.
   */
  async showWip(cwd: string): Promise<void> {
    const sel = this.selection;
    const row = cwd === this.id ? (this.graph.summary?.wipRow ?? null) : null;
    if (sel.kind === 'wip' && sel.cwd === cwd && this.graph.selectedRow === row) return;
    if (row != null) await this.selectRow(row);
    else this.select({ kind: 'wip', cwd });
  }

  /**
   * After a reset: when the selected commit `sel` left the graph with it (the usual "undo the last
   * commit"), select the WIP row — what the reset kept — else HEAD's row. A commit that is still
   * there, or a selection changed meanwhile, stays.
   */
  async leaveGoneCommit(sel: Selection): Promise<void> {
    if (sel.kind !== 'commit' || this.selection !== sel) return;
    const row = await this.graph.find(sel.oid);
    if (row != null || this.selection !== sel) return;
    const s = this.graph.summary;
    const next = s?.wipRow ?? s?.headRow;
    if (next != null) await this.selectRow(next);
  }

  /** Select a commit by oid, scrolling the graph to it when loaded. Returns false if not in the graph. */
  async reveal(oid: string): Promise<boolean> {
    const seq = ++this.#selSeq;
    const i = await this.graph.find(oid);
    if (seq !== this.#selSeq) return false;
    if (i == null) {
      this.graph.selectedRow = null;
      this.graph.compareRow = null;
      this.selection = { kind: 'commit', oid };
      return false;
    }
    await this.selectRow(i, 'center');
    return true;
  }

  /** Oid of the selection's graph row ('' = the opened worktree's WIP row); a compare: its `to` side. */
  #selectedOid(sel: Selection): string | null {
    switch (sel.kind) {
      case 'commit':
      case 'stash':
        return sel.oid;
      case 'compare':
        return sel.to;
      case 'wip':
        return sel.cwd === this.id ? '' : null;
      default:
        return null;
    }
  }

  /**
   * Select rows a and b (oid '' = the WIP row) as a compare. Higher row index = older = `from`, but the
   * working tree is always `to`. Invariant (shared with CompareDetails' swap and #resyncSelection):
   * graph.selectedRow is the `to` row, graph.compareRow the `from` row.
   */
  #compare(a: number, aOid: string, b: number, bOid: string): void {
    const aOlder = aOid !== '' && (bOid === '' || a > b);
    this.graph.selectedRow = aOlder ? b : a;
    this.graph.compareRow = aOlder ? a : b;
    this.selection = aOlder ? { kind: 'compare', from: aOid, to: bOid } : { kind: 'compare', from: bOid, to: aOid };
  }

  /** ⌘-click: compare the selection (commit, stash, WIP, or the `to` side of a compare) with row i. */
  async compareWithRow(i: number): Promise<void> {
    const seq = ++this.#selSeq;
    const curOid = this.#selectedOid(this.selection);
    const cur = this.graph.selectedRow;
    const row = await this.graph.getRow(i);
    if (seq !== this.#selSeq) return;
    if (!row || curOid == null || cur == null || cur === i) return;
    this.#compare(cur, curOid, i, row.oid);
  }

  /**
   * ⇧-click / ⇧↑↓: the commits from the range anchor (else the selection) to row i, as a compare with a
   * CommitRange. The working tree or a stash at either end compares the two rows as ⌘-click does.
   * Row i alone is selected when it is the anchor or the anchor has no graph row.
   */
  async selectRange(i: number): Promise<void> {
    const seq = ++this.#selSeq;
    const g = this.graph;
    const gen = g.generation;
    const stale = () => seq !== this.#selSeq || g.generation !== gen;
    const sel = this.selection;
    const anchorOid =
      sel.kind === 'compare' && sel.range?.anchor === 'first' ? sel.range.first : this.#selectedOid(sel);
    const a = anchorOid == null ? null : await g.find(anchorOid);
    if (stale()) return;
    if (a == null || a === i) return this.selectRow(i);
    const [ra, ri] = await Promise.all([g.getRow(a), g.getRow(i)]);
    if (stale() || !ra || !ri) return;
    if (ra.kind === 'commit' && ri.kind === 'commit') {
      const [newRow, newer, oldRow, older] = a < i ? [a, ra, i, ri] : [i, ri, a, ra];
      const count = await this.#firstParentChain(newRow, newer.parents[0], oldRow, gen, stale);
      if (count === undefined) return;
      g.selectedRow = newRow;
      g.compareRow = oldRow;
      this.selection = {
        kind: 'compare',
        from: count == null ? older.oid : (older.parents[0] ?? emptyTree(older.oid)),
        to: newer.oid,
        range: { first: older.oid, count, anchor: a < i ? 'to' : 'first' },
      };
    } else {
      this.#compare(a, ra.oid, i, ri.oid);
    }
    g.scrollTo(i);
  }

  /**
   * ⇧↑ / ⇧↓: move the end opposite the anchor (a range's other end, a compare's partner, else the
   * selection itself) to the next commit row in `dir`. Stash rows are stepped over; the WIP row stops it.
   */
  async extendRange(dir: 1 | -1): Promise<void> {
    const seq = this.#selSeq;
    const g = this.graph;
    const sel = this.selection;
    const end = sel.kind === 'compare' && sel.range?.anchor !== 'first' ? g.compareRow : g.selectedRow;
    if (end == null) return;
    const read = rowReader(g, g.generation);
    for (let i = end + dir; ; i += dir) {
      const r = await read(i);
      if (seq !== this.#selSeq || !r || r.kind === 'wip') return;
      if (r.kind === 'commit') return this.selectRange(i);
    }
  }

  /**
   * Length of the first-parent chain from row `newer` (whose first parent is `want`) down to row
   * `older`, both counted; null when `older` is not on it. Children always come before their parents,
   * so one scan over the rows in between finds the chain. Undefined when `stale()` turns true meanwhile.
   */
  async #firstParentChain(
    newer: number,
    want: string | undefined,
    older: number,
    gen: number,
    stale: () => boolean,
  ): Promise<number | null | undefined> {
    const read = rowReader(this.graph, gen);
    let count = 1;
    for (let k = newer + 1; k <= older && want; k++) {
      const r = await read(k);
      if (stale()) return undefined;
      if (!r) return null;
      if (r.oid !== want) continue;
      count++;
      if (k === older) return count;
      want = r.parents[0];
    }
    return null;
  }

  // ───────────── branch / tag prompt ─────────────

  /** Open the name input at graph row `row` (the graph shows: a viewer over it closes). */
  #prompt(prompt: RefPrompt): void {
    if (this.view.kind !== 'graph') this.closeView();
    this.refPrompt = prompt;
    this.graph.scrollTo(prompt.row, 'center');
  }

  /**
   * Ask for a branch name at graph row `row`, the row of the commit `oid`. The toolbar's Branch asks
   * for HEAD's row with `checkout`; a row menu's "Create branch here" for that row without it.
   */
  promptBranch(row: number, oid: string, checkout: boolean): void {
    this.#prompt({ kind: 'branch', row, oid, checkout });
  }

  /** Ask for a tag name at graph row `row`, the row of the commit `oid` ("Create tag here"). */
  promptTag(row: number, oid: string): void {
    this.#prompt({ kind: 'tag', row, oid, checkout: false });
  }

  /** The toolbar's Branch: at the HEAD commit, switched to once created. Nothing while HEAD is unborn. */
  promptBranchAtHead(): void {
    const row = this.graph.summary?.headRow;
    const oid = this.status?.head.oid ?? this.refs?.head.oid;
    if (row == null || !oid) return;
    this.promptBranch(row, oid, true);
  }

  // ───────────── center view ─────────────

  /** Viewers opened on top of other viewers (diff → blame → …); `back()` returns through them. */
  #viewStack: CenterView[] = [];

  /** Open a viewer. Opening a different kind of viewer over a viewer remembers it for `back()`. */
  openView(v: CenterView): void {
    const cur = this.view;
    if (cur.kind !== 'graph' && cur.kind !== v.kind) {
      this.#viewStack.push(cur);
      if (this.#viewStack.length > 20) this.#viewStack.shift();
    }
    this.view = v;
  }

  /** Back one level: the previous viewer, else the graph. */
  back(): void {
    this.view = this.#viewStack.pop() ?? { kind: 'graph' };
  }

  /** Close every viewer and return to the graph. */
  closeView(): void {
    this.#viewStack = [];
    this.view = { kind: 'graph' };
  }

  openDiff(items: DiffItem[], index: number): void {
    if (items.length === 0) return;
    this.openView({ kind: 'diff', items, index: Math.max(0, Math.min(index, items.length - 1)) });
  }

  /** Move within the open diff's sibling list. */
  stepDiff(dir: 1 | -1): void {
    const v = this.view;
    if (v.kind !== 'diff') return;
    const index = v.index + dir;
    if (index < 0 || index >= v.items.length) return;
    this.view = { ...v, index };
  }

  // ───────────── ref visibility ─────────────

  #savePrefs(): void {
    saveJSON(`repo:${this.id}`, { hiddenRefs: this.hiddenRefs, onlyRefs: this.onlyRefs } satisfies RepoPrefs);
  }

  isHidden(fullName: string): boolean {
    return this.hiddenRefs.includes(fullName);
  }

  toggleHidden(fullName: string): void {
    this.hiddenRefs = this.isHidden(fullName)
      ? this.hiddenRefs.filter((r) => r !== fullName)
      : [...this.hiddenRefs, fullName];
    this.#savePrefs();
    void this.reloadGraph();
  }

  /** Hide or show many refs at once (sidebar section bulk action). */
  setHidden(fullNames: string[], hidden: boolean): void {
    const set = new Set(this.hiddenRefs);
    for (const n of fullNames) {
      if (hidden) set.add(n);
      else set.delete(n);
    }
    this.hiddenRefs = [...set];
    this.#savePrefs();
    void this.reloadGraph();
  }

  /** Show only this ref (toggle). */
  solo(fullName: string): void {
    this.onlyRefs = this.onlyRefs.length === 1 && this.onlyRefs[0] === fullName ? [] : [fullName];
    this.#savePrefs();
    void this.reloadGraph();
  }

  /** Show only these refs (replaces the solo set; empty = show everything). */
  soloMany(fullNames: string[]): void {
    this.onlyRefs = [...new Set(fullNames)];
    this.#savePrefs();
    void this.reloadGraph();
  }

  clearFilters(): void {
    this.hiddenRefs = [];
    this.onlyRefs = [];
    this.#savePrefs();
    void this.reloadGraph();
  }
}
