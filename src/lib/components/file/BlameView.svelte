<!--
  Blame of one file (`repo.view` kind 'blame'). Gutter per line: a 3px age bar (newer = brighter),
  and on the first line of each run of lines from one commit (or the first visible line) the short
  sha, author and date; hovering it highlights every line of that commit. Click → select the commit
  (the blame stays); ⌘-click → blame the revision before it. Re-blames form a local back/forward
  history (⌘[ / ⌘]); n/p (⌥↓/⌥↑) step through runs, Enter selects the current run's commit; w (or
  the bar toggle) re-blames ignoring whitespace (`-w`, the diff's setting). Esc / back: previous viewer.
  Git runs again only when an input changed: a working-tree blame's file text (read from disk on
  worktree events) or its worktree's HEAD / merge, the commit a symbolic revision resolves to. One
  blame runs at a time; the backend kills a superseded one.
-->
<script module lang="ts">
  import { LRU } from '../../state/cache';
  import type { BlameData } from '../../types';

  interface Spec {
    path: string;
    rev: string | null;
    cwd: string | null;
  }

  /** One entry of the local blame history. */
  interface Visit {
    spec: Spec;
    /** scrollTop when last left. */
    top: number | null;
    /** Line to center on first show (re-blame keeps the clicked line in view). */
    line: number | null;
  }

  /** Oid-pinned blames never change: keep a few for instant back/forward. */
  const blameCache = new LRU<string, Promise<BlameData>>(12);

  /**
   * The re-blame history of a blame left for another viewer (e.g. "Show change"), by its view object:
   * that viewer remembers the object and `repo.back()` brings the same object back to a new instance.
   */
  const leftAt = new WeakMap<object, { visits: Visit[]; at: number }>();
</script>

<script lang="ts">
  import ArrowLeft from '@lucide/svelte/icons/arrow-left';
  import ChevronLeft from '@lucide/svelte/icons/chevron-left';
  import ChevronRight from '@lucide/svelte/icons/chevron-right';
  import Copy from '@lucide/svelte/icons/copy';
  import FileDiff from '@lucide/svelte/icons/file-diff';
  import FileQuestionMark from '@lucide/svelte/icons/file-question-mark';
  import Hash from '@lucide/svelte/icons/hash';
  import Locate from '@lucide/svelte/icons/locate';
  import Pencil from '@lucide/svelte/icons/pencil';
  import ScanText from '@lucide/svelte/icons/scan-text';
  import TextQuote from '@lucide/svelte/icons/text-quote';
  import WholeWord from '@lucide/svelte/icons/whole-word';
  import { onMount, tick, untrack } from 'svelte';
  import { api, errorMessage } from '../../api';
  import type { Token } from '../../highlight';
  import { app } from '../../state/app.svelte';
  import { getCommitDetails } from '../../state/cache';
  import { getRepo, type DiffItem } from '../../state/repo.svelte';
  import { settings } from '../../state/settings.svelte';
  import { showContextMenu, toast } from '../../state/ui.svelte';
  import type { BlameCommit, BlameHunk, BlobSpec, DiffTarget, RefsSnapshot } from '../../types';
  import {
    absoluteDate,
    absoluteTime,
    basename,
    copyText,
    dirname,
    relativeTime,
    shortOid,
  } from '../../util/format';
  import { isComposing, isEditable } from '../../util/keys';
  import VirtualList from '../common/VirtualList.svelte';
  import DiffLineText from '../diff/DiffLineText.svelte';
  import {
    LINE_H,
    PENDING,
    escapeBack,
    highlightFor,
    isOidRev,
    maxColumns,
    sameLines,
    selectedText,
    shortRev,
    splitLines,
    stripCR,
    within,
  } from './FileViewer.svelte';

  interface Model {
    key: string;
    lines: string[];
    hunks: BlameHunk[];
    commits: Record<string, BlameCommit>;
    /** line → hunk index */
    hunkOf: Uint32Array;
    /** line → run index (a run = consecutive lines from one commit) */
    runOf: Uint32Array;
    /** first line of each run */
    runs: number[];
    /** oid → 0..1, newest = 1 */
    age: Map<string, number>;
    cols: number;
  }

  /** What a blame was started from besides the file text; `recheck` compares it with the current state. */
  interface Inputs {
    key: string;
    ws: boolean;
    /** `baseOf` the spec. */
    base: string;
    /** repo.worktreeVersion when the blame started, or when its file text was last found unchanged. */
    wt: number;
  }

  const GUTTER_W = 232;
  const HL_WAIT = 100;

  const repo = getRepo();
  /** Back to a blame left for another viewer: its re-blame history, scroll positions included. */
  const left = leftAt.get(repo.view);
  let model = $state.raw<Model | null>(null);
  let tokens = $state.raw<Token[][] | null>(null);
  let error = $state<string | null>(null);
  let loading = $state(false);
  let visits = $state.raw<Visit[]>(left?.visits ?? []);
  let at = $state(left?.at ?? -1);
  /** Commit whose lines are highlighted (gutter hover). */
  let hoverOid = $state<string | null>(null);
  /** Current run (keyboard / last click), -1 = none. */
  let cur = $state(-1);
  /** First fully visible line. */
  let top = $state(0);
  /** The list's virtual scrollTop, as last reported (on destroy the list is already detached). */
  let listTop = 0;
  /** The blame view object followed last (see leftAt). */
  let followed: object | null = null;
  let list: VirtualList | undefined = $state();
  let root: HTMLDivElement | undefined = $state();
  let seq = 0;
  /** Inputs of the newest blame started (null before the first). */
  let last: Inputs | null = null;
  /** The newest blame, or a check of the file text, is in flight: re-checks wait until it settles. */
  let busy = false;
  /** Set on destroy: late async results are dropped. */
  let gone = false;
  /** Bumped by every selection made from this view: a late reveal yields to a newer one. */
  let picks = 0;

  const spec = $derived<Spec | null>(visits[at]?.spec ?? null);
  const base = $derived(spec ? baseOf(spec) : '');
  const active = $derived(app.active?.repo === repo);
  const stale = $derived(loading && model != null && spec != null && model.key !== specKey(spec));
  const digits = $derived(model ? Math.max(3, String(model.lines.length).length) : 3);

  function specKey(s: Spec): string {
    return `${s.path}\0${s.rev ?? ''}\0${s.cwd ?? ''}`;
  }

  // Follow repo.view: our own navigation (visit/travel) already sits at visits[at]; anything else
  // (e.g. "blame" from a file list while this view is open) becomes a new history entry.
  $effect(() => {
    const v = repo.view;
    if (v.kind !== 'blame') return;
    followed = v;
    const s: Spec = { path: v.path, rev: v.rev, cwd: v.cwd };
    untrack(() => {
      const here = visits[at];
      if (!here || specKey(here.spec) !== specKey(s)) {
        remember();
        visits = [...visits.slice(0, at + 1), { spec: s, top: null, line: null }];
        at = visits.length - 1;
      }
      void load(visits[at]);
    });
  });

  // Keep the blame current, running git only when what it was computed from changed (see recheck):
  // the whitespace toggle re-blames in place (same lines, other attribution), a symbolic revision
  // (e.g. HEAD) follows the commit it resolves to, a working-tree blame its file text and HEAD.
  // A hidden tab catches up when it is shown again.
  $effect(() => {
    void repo.worktreeVersion;
    void base;
    void settings.ignoreWhitespace;
    if (active) untrack(recheck);
  });

  onMount(() => {
    const offEsc = escapeBack(repo);
    // ⌘[ / ⌘] walk the re-blame history from anywhere in the active tab (also after an error).
    const onWindowKey = (e: KeyboardEvent) => {
      if (!e.metaKey || e.shiftKey || e.altKey || e.ctrlKey || e.defaultPrevented || isComposing(e)) return;
      if (e.code !== 'BracketLeft' && e.code !== 'BracketRight') return;
      if (app.active?.repo !== repo || isEditable(e.target)) return;
      e.preventDefault();
      travel(e.code === 'BracketLeft' ? -1 : 1);
    };
    window.addEventListener('keydown', onWindowKey);
    return () => {
      gone = true;
      seq++;
      // A viewer opened over this one keeps its view object for back(): keep the history, with the
      // shown visit's scroll position.
      const v = visits[at];
      if (followed && v) {
        if (model?.key === specKey(v.spec)) v.top = listTop;
        leftAt.set(followed, { visits, at });
      }
      offEsc();
      window.removeEventListener('keydown', onWindowKey);
    };
  });

  function fetchBlame(s: Spec, ws: boolean): Promise<BlameData> {
    if (s.rev == null || !isOidRev(s.rev)) return api.blame(repo.id, s.path, s.rev, s.cwd, ws);
    const k = `${repo.id}\0${s.rev}\0${s.path}\0${ws ? 'w' : ''}`;
    const hit = blameCache.get(k);
    if (hit) return hit;
    const p = api.blame(repo.id, s.path, s.rev, s.cwd, ws);
    blameCache.set(k, p);
    // Failed or cancelled (superseded by another blame): the next request runs it again.
    p.catch(() => blameCache.delete(k, p));
    return p;
  }

  /**
   * What a blame of `s` depends on besides its spec and the file text. '' for a revision pinned to an
   * oid; HEAD and the working tree: the commit HEAD of the blamed worktree points at, plus for the
   * working tree a merge in progress (MERGE_HEAD becomes a parent of blame's working-tree commit); any
   * other symbolic revision: every ref.
   */
  function baseOf(s: Spec): string {
    const rev = s.rev;
    if (rev != null && isOidRev(rev)) return '';
    const refs = repo.refs;
    if (!refs) return '';
    if (rev != null && rev !== 'HEAD') return refsKey(refs);
    const cwd = s.cwd ?? repo.id;
    const head = (cwd === repo.id ? refs.head.oid : refs.worktrees.find((w) => w.path === cwd)?.head) ?? '';
    if (rev != null) return head;
    const op = cwd === repo.id ? repo.status?.operation : null;
    return op?.kind === 'merge' ? `${head}\0merge\0${op.detail ?? ''}` : head;
  }

  /** Every ref target (any of them can move a revision like `main~2`). */
  function refsKey(r: RefsSnapshot): string {
    const parts = [r.head.branch ?? '', r.head.oid ?? ''];
    for (const b of r.locals) parts.push(b.fullName, b.oid);
    for (const b of r.remotes) parts.push(b.fullName, b.oid);
    for (const t of r.tags) parts.push(t.fullName, t.oid);
    for (const s of r.stashes) parts.push(s.oid);
    for (const w of r.worktrees) parts.push(w.path, w.head ?? '');
    return parts.join('\0');
  }

  /**
   * Bring the blame up to date, running git only when needed: re-blame at once (superseding one in
   * flight) when the whitespace setting or `baseOf` changed; for a working-tree blame after worktree
   * events, only when the file text (read from disk, no git) differs from the shown lines. One blame
   * or text check runs at a time; events meanwhile are picked up when it settles.
   */
  function recheck(): void {
    const here = visits[at];
    const l = last;
    // Before the first blame, or while a navigation is under way (it loads the new spec itself).
    if (gone || !active || !here || !l || l.key !== specKey(here.spec)) return;
    const s = here.spec;
    if (l.ws !== settings.ignoreWhitespace || l.base !== baseOf(s)) {
      void load(here);
      return;
    }
    if (busy || s.rev != null || l.wt === repo.worktreeVersion) return;
    const m = model;
    if (m && m.key === l.key) void checkText(here, m, l);
    // The blame failed (e.g. a new file before `git add`): nothing to compare, blame again.
    else void load(here);
  }

  /** Re-blame the working-tree blame `m` of `v` only if the file text changed. */
  async function checkText(v: Visit, m: Model, l: Inputs): Promise<void> {
    const id = seq;
    const wt = repo.worktreeVersion;
    busy = true;
    let lines: string[] | null = null;
    try {
      const file: BlobSpec = { kind: 'worktree', cwd: v.spec.cwd ?? repo.id, path: v.spec.path };
      const b = await api.readBlob(repo.id, file, 'text');
      if (b.text != null) lines = splitLines(b.text);
    } catch {
      // Unreadable (deleted, ...): blame reports why.
    }
    // A newer blame started meanwhile: it owns `busy`.
    if (id !== seq) return;
    busy = false;
    if (!lines || !sameLines(lines, m.lines)) void load(v);
    else {
      l.wt = wt;
      recheck();
    }
  }

  /** The newest blame landed (or failed): pick up whatever changed meanwhile. */
  function settle(id: number): void {
    if (id !== seq) return;
    busy = false;
    recheck();
  }

  function build(d: BlameData, key: string): Model {
    const lines = stripCR(d.lines);
    const n = lines.length;
    const hunkOf = new Uint32Array(n);
    d.hunks.forEach((h, k) => hunkOf.fill(k, Math.min(h.start, n), Math.min(h.start + h.count, n)));
    const runOf = new Uint32Array(n);
    const runs: number[] = [];
    for (let i = 0; i < n; i++) {
      if (i === 0 || d.hunks[hunkOf[i]]?.oid !== d.hunks[hunkOf[i - 1]]?.oid) runs.push(i);
      runOf[i] = runs.length - 1;
    }
    // Rank by time so brightness spreads evenly over the file's commits.
    const commits = Object.values(d.commits);
    const times = [...new Set(commits.filter((c) => !c.uncommitted).map((c) => c.time))].sort((a, b) => a - b);
    const rank = new Map(times.map((t, i) => [t, i] as const));
    const age = new Map<string, number>();
    for (const c of commits) {
      age.set(c.oid, c.uncommitted || times.length < 2 ? 1 : (rank.get(c.time) ?? 0) / (times.length - 1));
    }
    return { key, lines, hunks: d.hunks, commits: d.commits, hunkOf, runOf, runs, age, cols: maxColumns(lines) };
  }

  async function load(v: Visit): Promise<void> {
    const k = specKey(v.spec);
    const id = ++seq;
    // Not part of the model key: a whitespace toggle keeps the scroll position like a refresh.
    const ws = settings.ignoreWhitespace;
    last = { key: k, ws, base: baseOf(v.spec), wt: repo.worktreeVersion };
    busy = true;
    loading = true;
    try {
      const data = await fetchBlame(v.spec, ws).catch((e: unknown) => {
        // Cancelled by a blame that was then dropped (a shared cached run), or by an older request that
        // reached the backend after this one: still wanted, so ask once more.
        if (id !== seq || errorMessage(e) !== 'cancelled') throw e;
        return fetchBlame(v.spec, ws);
      });
      if (id !== seq) return;
      const m = build(data, k);
      const prev = model;
      const same = prev != null && prev.key === k;
      let t: Token[][] | null = null;
      let late: Promise<Token[][] | null> | null = null;
      if (prev && same && tokens && sameLines(prev.lines, m.lines)) t = tokens;
      else {
        const hl = highlightFor(v.spec.path, m.lines);
        const early = await within(hl, HL_WAIT);
        if (id !== seq) return;
        if (early === PENDING) late = hl;
        else t = early;
      }
      model = m;
      tokens = t;
      error = null;
      loading = false;
      if (!same) {
        hoverOid = null;
        cur = -1;
        await tick();
        if (id !== seq) return;
        restoreScroll(v, m);
        if (!isEditable(document.activeElement)) list?.focus();
      } else if (prev && cur >= 0) {
        // Keep the current run at its first line: a whitespace toggle (or an edit) regroups the runs.
        const line = prev.runs[cur];
        cur = line !== undefined && line < m.lines.length ? m.runOf[line] : -1;
      }
      settle(id);
      if (late) {
        const lt = await late;
        if (id === seq) tokens = lt;
      }
    } catch (e) {
      if (id !== seq) return;
      model = null;
      tokens = null;
      error = errorMessage(e);
      loading = false;
      settle(id);
    }
  }

  function restoreScroll(v: Visit, m: Model): void {
    if (!list) return;
    const vp = list.getViewport();
    if (vp) vp.scrollLeft = 0;
    if (v.top != null) list.scrollToIndex(Math.round(v.top / LINE_H), 'start');
    else if (v.line != null && m.lines.length > 0) list.scrollToIndex(Math.min(v.line, m.lines.length - 1), 'center');
    else {
      // Blame at an exact commit (e.g. from its file list): land on the first lines it introduced.
      const rev = v.spec.rev;
      const r = rev ? m.runs.findIndex((start) => m.hunks[m.hunkOf[start]]?.oid === rev) : -1;
      if (r < 0) list.scrollToIndex(0, 'start');
      else {
        cur = r;
        list.scrollToIndex(Math.max(0, m.runs[r] - Math.floor(list.pageSize() / 3)), 'start');
      }
    }
    // No scroll event fires when the position did not change (or the list is new): sync `top` here.
    if (list) onScroll(list.getScrollTop());
  }

  /** Save the scroll position of the visit being left. */
  function remember(): void {
    const v = visits[at];
    if (v && list && model?.key === specKey(v.spec)) v.top = list.getScrollTop();
  }

  function visit(s: Spec, line: number | null): void {
    remember();
    visits = [...visits.slice(0, at + 1), { spec: s, top: null, line }];
    at = visits.length - 1;
    repo.openView({ kind: 'blame', path: s.path, rev: s.rev, cwd: s.cwd });
  }

  function travel(d: -1 | 1): void {
    const j = at + d;
    if (j < 0 || j >= visits.length) return;
    remember();
    at = j;
    const s = visits[j].spec;
    repo.openView({ kind: 'blame', path: s.path, rev: s.rev, cwd: s.cwd });
  }

  function hunkAt(m: Model, i: number): BlameHunk | undefined {
    return m.hunks[m.hunkOf[i]];
  }

  function commitAt(m: Model, i: number): BlameCommit | undefined {
    const h = hunkAt(m, i);
    return h ? m.commits[h.oid] : undefined;
  }

  /** Select the commit of line i in the graph / details (uncommitted lines → the working tree). */
  function revealLine(i: number): void {
    const m = model;
    const c = m ? commitAt(m, i) : undefined;
    if (!c) return;
    picks++;
    if (!c.uncommitted) {
      void repo.reveal(c.oid);
      return;
    }
    const cwd = spec?.cwd ?? repo.id;
    const wip = repo.graph.summary?.wipRow;
    if (cwd === repo.id && wip != null) void repo.selectRow(wip, 'center');
    else repo.select({ kind: 'wip', cwd });
  }

  /** Blame the revision before the commit that last changed line i, keeping that line in view. */
  async function blamePrior(i: number): Promise<void> {
    const m = model;
    const s = spec;
    if (!m || !s) return;
    const h = hunkAt(m, i);
    if (!h) return;
    const c = m.commits[h.oid];
    if (!c || c.uncommitted) {
      visit({ path: s.path, rev: 'HEAD', cwd: s.cwd }, i);
      return;
    }
    // Every navigation (Esc, ⌘[, another blame) assigns a new repo.view: drop the result then.
    const v = repo.view;
    const moved = () => gone || repo.view !== v;
    const path = h.origPath ?? s.path;
    let rev = `${c.oid}^`;
    let prevPath = path;
    try {
      const d = await getCommitDetails(repo.id, c.oid);
      if (moved()) return;
      if (d.parents.length === 0) {
        toast('Root commit');
        return;
      }
      const f = d.files.find((x) => x.path === path);
      if (f?.status === 'A') {
        toast(`Added in ${shortOid(c.oid)}`);
        return;
      }
      rev = d.parents[0];
      prevPath = f?.oldPath ?? path;
    } catch {
      // Details unavailable: let git resolve `<oid>^` and report any error in place.
    }
    if (moved()) return;
    visit({ path: prevPath, rev, cwd: null }, h.origLine - 1 + (i - h.start));
  }

  /** Open the diff of the commit that last changed line i, positioned on this file. */
  async function openChange(i: number): Promise<void> {
    const m = model;
    const s = spec;
    const h = m ? hunkAt(m, i) : undefined;
    const c = m && h ? m.commits[h.oid] : undefined;
    if (!s || !h || !c || c.uncommitted) return;
    const path = h.origPath ?? s.path;
    // Every navigation (Esc, ⌘[, another blame) assigns a new repo.view: drop the result then.
    const v = repo.view;
    const moved = () => gone || repo.view !== v;
    try {
      const d = await getCommitDetails(repo.id, c.oid);
      if (moved()) return;
      const index = d.files.findIndex((f) => f.path === path);
      if (index < 0) {
        toast(`Unchanged in ${shortOid(c.oid)}`);
        return;
      }
      const target: DiffTarget = { kind: 'commit', oid: c.oid, parent: null };
      const items: DiffItem[] = d.files.map((f) => ({
        target,
        file: { path: f.path, oldPath: f.oldPath, status: f.status },
      }));
      repo.openDiff(items, index);
    } catch (e) {
      if (!moved()) toast(errorMessage(e));
    }
  }

  /** Select the blamed revision's commit (the bar's rev button). */
  async function revealRev(): Promise<void> {
    const s = spec;
    const r = s?.rev;
    if (!s || !r) return;
    const pick = ++picks;
    if (r === 'HEAD') {
      // HEAD of the blamed worktree (e.g. "Blame HEAD" from uncommitted lines).
      const cwd = s.cwd ?? repo.id;
      const head = cwd === repo.id ? repo.refs?.head.oid : repo.refs?.worktrees.find((w) => w.path === cwd)?.head;
      if (head) void repo.reveal(head);
      return;
    }
    const m = /^([0-9a-f]{40,64})(\^1?)?$/i.exec(r);
    if (!m) return;
    if (!m[2]) {
      void repo.reveal(m[1]);
      return;
    }
    const v = repo.view;
    const sel = repo.selection;
    const parent = await firstParent(m[1]);
    // Dropped when the user moved on meanwhile: a newer selection (a gutter click here, or elsewhere)
    // or another view.
    if (!parent || gone || pick !== picks || repo.selection !== sel || repo.view !== v) return;
    void repo.reveal(parent);
  }

  /** First parent of `oid`: from its graph row when the graph has it (no git), else from its details. */
  async function firstParent(oid: string): Promise<string | null> {
    const g = repo.graph;
    const i = await g.find(oid);
    const row = i == null ? undefined : await g.getRow(i);
    if (row?.oid === oid) return row.parents[0] ?? null;
    try {
      return (await getCommitDetails(repo.id, oid)).parents[0] ?? null;
    } catch {
      return null;
    }
  }

  function copy(text: string): void {
    void copyText(text).then(() => toast('Copied'));
  }

  /** Shared with the diff's whitespace toggle; the effect above re-blames. */
  function toggleWhitespace(): void {
    settings.ignoreWhitespace = !settings.ignoreWhitespace;
  }

  function onGutterClick(e: MouseEvent, i: number): void {
    const m = model;
    if (!m) return;
    cur = m.runOf[i];
    list?.focus();
    if (e.metaKey) void blamePrior(i);
    else revealLine(i);
  }

  function gutterMenu(e: MouseEvent, i: number): void {
    const m = model;
    const c = m ? commitAt(m, i) : undefined;
    if (!m || !c) return;
    cur = m.runOf[i];
    if (c.uncommitted) {
      showContextMenu(e, [
        { label: 'Select working tree', icon: Locate, action: () => revealLine(i) },
        { label: 'Blame HEAD', icon: ScanText, hint: '⌘-click', action: () => void blamePrior(i) },
      ]);
      return;
    }
    showContextMenu(e, [
      { label: 'Select commit', icon: Locate, action: () => revealLine(i) },
      { label: 'Blame previous revision', icon: ScanText, hint: '⌘-click', action: () => void blamePrior(i) },
      { label: 'Show change', icon: FileDiff, action: () => void openChange(i) },
      { separator: true },
      { label: 'Copy SHA', icon: Copy, action: () => copy(c.oid) },
      { label: 'Copy short SHA', icon: Hash, action: () => copy(shortOid(c.oid)) },
      { label: 'Copy summary', icon: TextQuote, action: () => copy(c.summary) },
    ]);
  }

  /** Move the current run by `dir` and bring it to the upper third of the viewport. */
  function step(m: Model, dir: 1 | -1): void {
    if (!list || m.runs.length === 0 || m.lines.length === 0) return;
    let r: number;
    if (cur >= 0 && cur < m.runs.length) r = cur + dir;
    else {
      const anchor = Math.max(0, Math.min(m.lines.length - 1, top + Math.floor(list.pageSize() / 3)));
      const a = m.runOf[anchor];
      r = dir > 0 ? a + 1 : m.runs[a] < anchor ? a : a - 1;
    }
    r = Math.max(0, Math.min(m.runs.length - 1, r));
    cur = r;
    list.scrollToIndex(Math.max(0, m.runs[r] - Math.floor(list.pageSize() / 3)), 'start');
  }

  function onKey(e: KeyboardEvent): void {
    if (isComposing(e)) return;
    const m = model;
    if (!m) return;
    const plain = !e.metaKey && !e.ctrlKey && !e.altKey;
    if ((plain && e.code === 'KeyN') || (e.altKey && e.code === 'ArrowDown')) step(m, 1);
    else if ((plain && e.code === 'KeyP') || (e.altKey && e.code === 'ArrowUp')) step(m, -1);
    else if (plain && !e.shiftKey && e.code === 'KeyW') {
      // A toggle: key auto-repeat would flip it back and forth.
      if (!e.repeat) toggleWhitespace();
    } else if (e.code === 'Enter' && cur >= 0 && cur < m.runs.length) {
      if (e.metaKey) void blamePrior(m.runs[cur]);
      else revealLine(m.runs[cur]);
    } else return;
    e.preventDefault();
  }

  function onScroll(scrollTop: number): void {
    listTop = scrollTop;
    top = Math.ceil(scrollTop / LINE_H);
  }

  function tip(c: BlameCommit): string {
    return c.uncommitted
      ? 'Uncommitted'
      : `${shortOid(c.oid)} ${c.summary}\n${c.author} <${c.email}>\n${absoluteTime(c.time)}`;
  }

  function dateText(t: number): string {
    return settings.dateFormat === 'relative' ? relativeTime(t) : absoluteDate(t);
  }

  function oncopy(e: ClipboardEvent): void {
    const m = model;
    if (!m || !root) return;
    const text = selectedText(root, m.lines);
    if (text == null) return;
    e.clipboardData?.setData('text/plain', text);
    e.preventDefault();
  }
</script>

<svelte:document {oncopy} />

<div class="bv">
  <div class="panel-header bar">
    <button class="icon-btn" title="Back (Esc)" onclick={() => repo.back()}>
      <ArrowLeft size={14} strokeWidth={1.75} />
    </button>
    {#if visits.length > 1}
      <button class="icon-btn" title="Previous revision (⌘[)" disabled={at <= 0} onclick={() => travel(-1)}>
        <ChevronLeft size={14} strokeWidth={1.75} />
      </button>
      <button
        class="icon-btn"
        title="Next revision (⌘])"
        disabled={at >= visits.length - 1}
        onclick={() => travel(1)}
      >
        <ChevronRight size={14} strokeWidth={1.75} />
      </button>
    {/if}
    <span class="mode" title="Blame"><ScanText size={14} strokeWidth={1.75} /></span>
    {#if spec}
      {@const dir = dirname(spec.path)}
      <span class="path selectable" title={spec.path}
        >{#if dir}<span class="dir">{dir}/</span>{/if}<span class="name">{basename(spec.path)}</span></span
      >
      {#if spec.rev}
        <button class="rev" title={spec.rev} onclick={() => void revealRev()}>{shortRev(spec.rev)}</button>
      {:else}
        <span class="wt" title="Working tree"><Pencil size={12} strokeWidth={1.75} /></span>
      {/if}
    {/if}
    <span class="grow"></span>
    <button
      class="icon-btn"
      class:on={settings.ignoreWhitespace}
      title="Ignore whitespace (w)"
      onclick={toggleWhitespace}
    >
      <WholeWord size={14} strokeWidth={1.75} />
    </button>
  </div>
  <div class="body" bind:this={root}>
    {#if loading}<div class="progress"></div>{/if}
    {#if model && model.lines.length > 0}
      {@const m = model}
      <div class="code-list" class:stale style:--digits={digits} style:--cols={m.cols} style:--gutter-w="{GUTTER_W}px">
        <VirtualList
          bind:this={list}
          count={m.lines.length}
          rowHeight={LINE_H}
          overscan={16}
          horizontal
          tabindex={0}
          onscroll={onScroll}
          onkeydown={onKey}
        >
          {#snippet row(i)}
            {@const h = m.hunks[m.hunkOf[i]]}
            {@const c = h ? m.commits[h.oid] : undefined}
            {@const run = m.runOf[i]}
            {@const first = m.runs[run] === i}
            <div
              class="line"
              class:sep={first && i > 0}
              class:hl={h != null && h.oid === hoverOid}
              class:cur={run === cur}
              data-line={i}
            >
              <div class="fixed">
                <button
                  class="gutter"
                  tabindex="-1"
                  title={c ? tip(c) : undefined}
                  onclick={(e) => onGutterClick(e, i)}
                  oncontextmenu={(e) => gutterMenu(e, i)}
                  onmouseenter={() => (hoverOid = h?.oid ?? null)}
                  onmouseleave={() => (hoverOid = null)}
                >
                  <span class="age" style:opacity={0.15 + 0.85 * (h ? (m.age.get(h.oid) ?? 0) : 0)}></span>
                  {#if c && (first || i === top)}
                    {#if c.uncommitted}
                      <span class="wip"><Pencil size={11} strokeWidth={1.75} /></span>
                    {:else}
                      <span class="sha">{shortOid(c.oid)}</span>
                      <span class="author">{c.author}</span>
                      <span class="date">{dateText(c.time)}</span>
                    {/if}
                  {/if}
                </button>
                <span class="ln" data-n={i + 1}></span>
              </div>
              <span class="code selectable" data-code
                ><DiffLineText text={m.lines[i] ?? ''} tokens={tokens?.[i] ?? null} ranges={null} /></span
              >
            </div>
          {/snippet}
        </VirtualList>
      </div>
    {:else if model}
      <div class="ph empty"><span class="glyph">∅</span></div>
    {:else if error}
      <div class="ph empty">
        <FileQuestionMark size={28} strokeWidth={1.5} />
        <span class="msg selectable">{error}</span>
      </div>
    {/if}
  </div>
</div>

<style>
  .bv {
    display: flex;
    flex-direction: column;
    height: 100%;
    min-height: 0;
    background: var(--bg-1);
  }
  .bar {
    height: var(--viewer-bar-h);
    gap: 6px;
  }
  .mode {
    display: inline-flex;
    flex: none;
    color: var(--text-3);
  }
  .path {
    min-width: 0;
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
    font-size: var(--fs-sm);
  }
  .dir {
    color: var(--text-2);
  }
  .name {
    color: var(--text-0);
  }
  .rev {
    flex: none;
    height: 20px;
    padding: 0 6px;
    border-radius: var(--radius-sm);
    background: var(--bg-3);
    color: var(--text-1);
    font-family: var(--font-mono);
    font-size: var(--fs-xs);
  }
  .rev:hover {
    color: var(--text-0);
  }
  .wt {
    display: inline-flex;
    flex: none;
    color: var(--text-2);
  }
  .body {
    position: relative;
    flex: 1;
    min-height: 0;
  }
  .code-list {
    position: absolute;
    inset: 0;
    font-family: var(--font-mono);
    font-size: var(--fs-code);
    line-height: var(--line-h);
    --ln-w: calc(var(--digits) * 1ch + 20px);
  }
  .line {
    display: flex;
    height: 100%;
    min-width: calc(var(--gutter-w) + var(--ln-w) + var(--cols) * 1ch + 24px);
  }
  .fixed {
    position: sticky;
    left: 0;
    z-index: 1;
    display: flex;
    flex: none;
    background: var(--bg-1);
  }
  .gutter {
    position: relative;
    display: flex;
    align-items: center;
    gap: 6px;
    width: var(--gutter-w);
    height: 100%;
    padding: 0 8px 0 11px;
    border-right: 1px solid var(--border);
    font-family: var(--font-ui);
    font-size: var(--fs-xs);
    color: var(--text-2);
    text-align: left;
    white-space: nowrap;
    overflow: hidden;
  }
  .age {
    position: absolute;
    top: 0;
    bottom: 0;
    left: 0;
    width: 3px;
    background: var(--accent);
  }
  .sha {
    flex: none;
    font-family: var(--font-mono);
    color: var(--text-2);
  }
  .author {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    color: var(--text-1);
  }
  .date {
    flex: none;
    margin-left: auto;
    color: var(--text-2);
    font-variant-numeric: tabular-nums;
  }
  .wip {
    display: inline-flex;
    color: var(--text-2);
  }
  .ln {
    flex: none;
    width: var(--ln-w);
    padding-right: 12px;
    text-align: right;
    color: var(--text-2);
  }
  .ln::before {
    content: attr(data-n);
  }
  .code {
    flex: 1 0 auto;
    padding-left: 4px;
    white-space: pre;
    tab-size: 4;
    color: var(--shiki-foreground);
  }
  .sep .gutter,
  .sep .ln {
    box-shadow: inset 0 1px 0 var(--border);
  }
  .hl,
  .hl .fixed {
    background: var(--bg-hover);
  }
  .cur .fixed {
    background: var(--bg-active);
  }
  .gutter:hover .sha {
    color: var(--accent);
  }
  .ph {
    position: absolute;
    inset: 0;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: var(--sp-2);
    padding: var(--sp-4);
    color: var(--text-3);
  }
  .msg {
    max-width: 560px;
    color: var(--text-2);
    font-size: var(--fs-xs);
    text-align: center;
    white-space: pre-wrap;
    word-break: break-word;
  }
  .glyph {
    font-size: 20px;
  }
  .stale {
    opacity: 0.55;
    transition: opacity 0s linear 150ms;
  }
  .progress {
    position: absolute;
    top: 0;
    left: 0;
    right: 0;
    height: 2px;
    z-index: 3;
    overflow: hidden;
    pointer-events: none;
    opacity: 0;
    animation: appear 0s linear 300ms forwards;
  }
  .progress::after {
    content: '';
    position: absolute;
    top: 0;
    bottom: 0;
    left: 0;
    width: 30%;
    background: var(--accent);
    animation: slide 1s ease-in-out infinite;
  }
  @keyframes appear {
    to {
      opacity: 1;
    }
  }
  @keyframes slide {
    from {
      transform: translateX(-100%);
    }
    to {
      transform: translateX(340%);
    }
  }
</style>
