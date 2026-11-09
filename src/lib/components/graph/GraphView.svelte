<!--
  Commit graph: a column header (upper-cased names; a gear at its right end opens the column picker)
  over virtualized rows [branch / tag | graph | subject | author | date | sha] over the lanes SVG, with
  the floating find box, scroll markers and column resize handles. No toolbar. A wheel over the header
  or a handle scrolls the list (they lie outside its scroller).
  The subject keeps at least MIN_SUBJECT_W: in a narrow list the date column gives way first (a time
  slot with time marks while there is room for one), then the author column (avatar only); narrower
  still, the rows scroll sideways while the columns right of the subject stick to the list's right edge
  (geometry.ts layoutColumns).
  A row's ref labels sit in the branch / tag column with a faint line to the row's node (or, with the
  column off, inline before the subject); hovering a collapsed group expands it (RefPopover).
  Mouse/keyboard selection drives repo.selectRow (⌘-press compares with the selection, ⇧-press and
  ⇧↑↓ select a range); ⌘C copies the selected sha via the DOM copy event.
-->
<script lang="ts">
  import { untrack } from 'svelte';
  import Archive from '@lucide/svelte/icons/archive';
  import ArchiveArrowUp from '@lucide/svelte/icons/archive-arrow-up';
  import ArchiveX from '@lucide/svelte/icons/archive-x';
  import Check from '@lucide/svelte/icons/check';
  import ChevronsDown from '@lucide/svelte/icons/chevrons-down';
  import Clock from '@lucide/svelte/icons/clock';
  import Columns3Cog from '@lucide/svelte/icons/columns-3-cog';
  import Copy from '@lucide/svelte/icons/copy';
  import FolderTree from '@lucide/svelte/icons/folder-tree';
  import Funnel from '@lucide/svelte/icons/funnel';
  import GitBranchPlus from '@lucide/svelte/icons/git-branch-plus';
  import GitCompare from '@lucide/svelte/icons/git-compare';
  import Hash from '@lucide/svelte/icons/hash';
  import MessageSquareText from '@lucide/svelte/icons/message-square-text';
  import TextQuote from '@lucide/svelte/icons/text-quote';
  import User from '@lucide/svelte/icons/user';
  import StatusIcon from '../common/StatusIcon.svelte';
  import VirtualList from '../common/VirtualList.svelte';
  import Avatar from '../details/Avatar.svelte';
  import ColumnPicker from './ColumnPicker.svelte';
  import GraphLanes from './GraphLanes.svelte';
  import GraphSearch from './GraphSearch.svelte';
  import RefBadges from './RefBadges.svelte';
  import ScrollMarkers from './ScrollMarkers.svelte';
  import TimeMarks from './TimeMarks.svelte';
  import { COLUMN_TOGGLES } from './columns';
  import {
    bandStart,
    graphWidth,
    layoutColumns,
    MAX_REFS_W,
    MIN_GRAPH_W,
    MIN_REFS_W,
    MIN_SUBJECT_W,
    naturalWidth,
    refsCellWidth,
    refsWidth,
    ROW_H,
    TIME_SLOT_W,
  } from './geometry';
  import { api, errorMessage } from '../../api';
  import { app } from '../../state/app.svelte';
  import { getCommitDetails, getDiffFiles } from '../../state/cache';
  import { CHUNK, type SearchState } from '../../state/graph.svelte';
  import { getRepo, rowReader, type DiffItem, type FileRef } from '../../state/repo.svelte';
  import { DEFAULT_SETTINGS, settings } from '../../state/settings.svelte';
  import { contextMenu, showContextMenu, toast, type MenuEntry } from '../../state/ui.svelte';
  import { absoluteTime, copyText, formatTime, shortOid } from '../../util/format';
  import { isComposing, isEditable } from '../../util/keys';
  import type { ChangeStatus, DiffTarget, FileChange, GraphRow, SearchMode, StatusEntry, WorkingStatus } from '../../types';

  const repo = getRepo();
  const graph = repo.graph;

  const OVERSCAN = 12;
  /** Rows fetched beyond the rendered window on each side. */
  const PREFETCH = 128;
  /** The window jumped further than this (scrollbar drag): fetch once the scroll settles, not every frame. */
  const JUMP_ROWS = CHUNK;
  const SETTLE_MS = 50;
  /** How many rows ⌥↑ looks upwards for a child of the selected commit. */
  const CHILD_SCAN = 4096;

  type ColKey = 'colRefs' | 'colGraph' | 'colAuthor' | 'colDate' | 'colSha';
  /** Width limits of the right-anchored columns. */
  const COL_LIMITS: Record<Exclude<ColKey, 'colRefs' | 'colGraph'>, [number, number]> = {
    colAuthor: [48, 400],
    colDate: [36, 220],
    colSha: [44, 200],
  };

  let list: VirtualList | undefined = $state();
  let range = $state({ start: 0, end: 0 });
  let listFocused = $state(false);
  /** Row width of the list (viewport client width, scrollbar excluded). */
  let listWidth = $state(0);
  let listHeight = $state(0);
  let resizing = $state<ColKey | null>(null);

  let searchOpen = $state(false);
  let searchText = $state('');
  let searchMode = $state<SearchMode>('quick');
  let searchFocus = $state(0);

  let pickerOpen = $state(false);
  let gear: HTMLButtonElement | undefined = $state();
  /** Text of the branch-name input (repo.branchPrompt) while it is open. */
  let branchName = $state('');
  /** The list's scrollLeft: how far the rows scrolled sideways. */
  let hscroll = $state(0);

  const total = $derived(graph.total);
  const truncated = $derived(total > 0 && !!graph.summary?.truncated);
  /** Rows of the list: the graph plus one trailing "history truncated" row. */
  const count = $derived(total + (truncated ? 1 : 0));
  /** Scroll markers annotate the scrollbar, so only while there is one. */
  const scrollable = $derived(count * ROW_H > listHeight);
  const maxLanes = $derived(graph.summary?.maxLanes ?? 0);
  const graphW = $derived(graphWidth(maxLanes, settings.colGraph));
  /** Branch / tag column width; 0 while the ref labels sit inline before the subject. */
  const refsW = $derived(settings.refsColumn ? refsWidth(settings.colRefs) : 0);
  /** Badges a branch / tag cell shows before its "+N" chip: by column width, never measured. */
  const refsMax = $derived(refsW < 200 ? 1 : refsW < 300 ? 2 : 3);
  /** The columns right of the graph for the measured list width (nothing gives way until measured). */
  const layout = $derived(
    layoutColumns(listWidth > 0 ? listWidth - refsW - graphW : Infinity, {
      author: settings.showAuthor ? settings.colAuthor : null,
      date: settings.showDate ? settings.colDate : null,
      sha: settings.showSha ? settings.colSha : null,
      timeSlot: TIME_SLOT_W[settings.dateFormat],
    }),
  );
  /** The tracks right of the subject: author, date or time slot, sha (each only while shown). */
  const rightCols = $derived(
    [layout.author, layout.date || layout.time, layout.sha]
      .filter((w) => w > 0)
      .map((w) => `${w}px`)
      .join(' '),
  );
  /** Row grid right of the graph: the subject's minimum guards it; the rows widen past the list if needed. */
  const cols = $derived([`minmax(${MIN_SUBJECT_W}px, 1fr)`, rightCols].filter(Boolean).join(' '));
  /** Width of the rows; past the list's width they scroll sideways. */
  const contentW = $derived(refsW + graphW + layout.subject + layout.right);
  /** Header names that scroll with the rows: the branch / tag, graph and subject tracks. */
  const headCols = $derived(`${refsW ? `${refsW}px ` : ''}${graphW}px ${layout.subject}px`);
  /** How far the rows scrolled sideways, within the overflow (a widened list clamps scrollLeft later). */
  const shift = $derived(Math.min(hscroll, Math.max(0, contentW - listWidth)));
  /** Sticky offsets from the list's right edge: each cell right of the subject keeps the cells after it. */
  const stickAuthor = $derived(layout.date + layout.time + layout.sha);
  const stickDate = $derived(layout.sha);
  /** Null while the matches belong to another generation (a reload's re-run is pending). */
  const currentMatch = $derived(graph.currentMatch);
  /** GitKraken-style counts: pencil / plus / minus / conflicts. A tracked path is counted once across
   *  staged and unstaged: deleted when either side deletes it (`MD`, `AD`: the file is gone from the
   *  disk), else added when either side adds it (`AM`: new files keep the plus after staging), else
   *  changed (renames, copies and type changes included). Untracked files add to the plus; a staged
   *  deletion whose path is back on disk untracked (`git rm --cached`) counts on both sides, like its
   *  two file-list rows. */
  const wip = $derived.by(() => {
    const s = repo.status;
    if (!s) return { changed: 0, added: 0, deleted: 0, conflicts: 0 };
    const tracked = new Map<string, ChangeStatus>();
    const rank = (st: ChangeStatus) => (st === 'D' ? 2 : st === 'A' ? 1 : 0);
    for (const entries of [s.staged, s.unstaged]) {
      for (const e of entries) {
        const prev = tracked.get(e.path);
        if (prev === undefined || rank(e.status) > rank(prev)) tracked.set(e.path, e.status);
      }
    }
    let changed = 0;
    let added = s.untracked.length;
    let deleted = 0;
    for (const st of tracked.values()) {
      if (st === 'D') deleted++;
      else if (st === 'A') added++;
      else changed++;
    }
    return { changed, added, deleted, conflicts: s.conflicted.length };
  });
  const filtered = $derived(repo.hiddenRefs.length > 0 || repo.onlyRefs.length > 0);
  const filterTitle = $derived.by(() => {
    const short = (n: string) => n.replace(/^refs\/(heads|remotes|tags)\//, '');
    const lines = [
      ...repo.onlyRefs.map((n) => `only ${short(n)}`),
      ...repo.hiddenRefs.map((n) => `hidden ${short(n)}`),
    ];
    const extra = lines.length > 20 ? [`+${lines.length - 20}`] : [];
    return ['Clear filters', ...lines.slice(0, 20), ...extra].join('\n');
  });
  /** Local branches checked out in another worktree: full ref name → worktree path (a folder mark). */
  const elsewhere = $derived.by(() => {
    const out = new Map<string, string>();
    for (const b of repo.refs?.locals ?? []) if (b.worktree && !b.isHead) out.set(b.fullName, b.worktree);
    return out;
  });
  /** Column boundaries in list px: the branch / tag column's and the graph's right edges (they scroll with
   *  the rows; dropped once scrolled out), then the left edge of each resizable column right of the subject
   *  (sticky at the right edge; none for the avatar-only author column and the time slot: fixed widths). */
  const handles = $derived.by(() => {
    const out: { key: ColKey; x: number }[] = [];
    if (refsW) out.push({ key: 'colRefs', x: refsW - shift });
    out.push({ key: 'colGraph', x: refsW + graphW - shift });
    let x = listWidth - layout.right;
    if (layout.author && !layout.compactAuthor) out.push({ key: 'colAuthor', x });
    x += layout.author;
    if (layout.date) out.push({ key: 'colDate', x });
    x += layout.date + layout.time;
    if (layout.sha) out.push({ key: 'colSha', x });
    return out.filter((h) => h.x >= 0);
  });

  // ───────────── wiring to the stores ─────────────

  // Fetch the rendered window plus a margin. Dragging the scrollbar over a big history moves the window
  // by thousands of rows per frame: fetching every position passed would flood IPC and re-render on each
  // arrival, so a jump waits until the scroll settles (keyboard targets are fetched by select()).
  let ensuredStart = 0;
  $effect(() => {
    const { start, end } = range;
    if (!graph.summary) return;
    const jump = Math.abs(start - ensuredStart) > JUMP_ROWS;
    ensuredStart = start;
    const fetch = () => graph.ensure(start - PREFETCH, end + PREFETCH);
    if (!jump) {
      fetch();
      return;
    }
    const timer = setTimeout(fetch, SETTLE_MS);
    return () => clearTimeout(timer);
  });

  let lastScroll = 0;
  $effect(() => {
    const req = graph.scrollRequest;
    const l = list;
    if (!req || !l || req.seq === lastScroll) return;
    lastScroll = req.seq;
    l.scrollToIndex(req.row, req.align);
  });

  let lastFind = untrack(() => repo.findRequest);
  $effect(() => {
    const n = repo.findRequest;
    if (n === lastFind) return;
    lastFind = n;
    untrack(openSearch);
  });

  // Take keyboard focus when this graph becomes the visible one and nothing else holds focus.
  $effect(() => {
    if (app.active?.repo !== repo || !repo.ready || repo.view.kind !== 'graph') return;
    const raf = requestAnimationFrame(() => {
      const a = document.activeElement;
      if (!a || a === document.body) list?.focus();
    });
    return () => cancelAnimationFrame(raf);
  });

  $effect(() => {
    const vp = list?.getViewport();
    if (!vp) return;
    const measure = () => {
      listWidth = vp.clientWidth;
      listHeight = vp.clientHeight;
    };
    const ro = new ResizeObserver(measure);
    ro.observe(vp);
    measure();
    // Sideways scrolling moves the header's names along (the sticky cells follow by themselves).
    const onScroll = () => {
      hscroll = vp.scrollLeft;
    };
    vp.addEventListener('scroll', onScroll, { passive: true });
    return () => {
      ro.disconnect();
      vp.removeEventListener('scroll', onScroll);
    };
  });

  function isActive(): boolean {
    return app.active?.repo === repo && repo.view.kind === 'graph';
  }

  // ───────────── branch prompt ─────────────

  // A new prompt (another commit) starts with an empty name; a relocated one keeps its text.
  let promptOid: string | null = null;
  $effect(() => {
    const oid = repo.branchPrompt?.oid ?? null;
    if (oid === promptOid) return;
    promptOid = oid;
    untrack(() => (branchName = ''));
  });

  // The prompt follows its commit across reloads (rows shift); a commit gone from the graph closes it.
  $effect(() => {
    const p = repo.branchPrompt;
    const gen = graph.generation;
    if (!p) return;
    untrack(() => {
      void graph.find(p.oid).then((row) => {
        if (repo.branchPrompt !== p || graph.generation !== gen) return;
        if (row == null) repo.branchPrompt = null;
        else if (row !== p.row) repo.branchPrompt = { ...p, row };
      });
    });
  });

  /** Focus a freshly rendered input (the branch-name prompt). */
  function autofocus(el: HTMLInputElement): void {
    el.focus();
    el.select();
  }

  function submitBranch(): void {
    const p = repo.branchPrompt;
    const name = branchName.trim();
    if (!p || !name) return;
    repo.branchPrompt = null;
    list?.focus();
    void repo.ops.createBranch(name, p.oid, p.checkout);
  }

  function cancelBranch(): void {
    if (!repo.branchPrompt) return;
    repo.branchPrompt = null;
    list?.focus();
  }

  /** Keys typed in the branch input never reach the list's shortcuts; Enter creates, Esc cancels. */
  function onBranchKey(e: KeyboardEvent): void {
    e.stopPropagation();
    if (isComposing(e)) return;
    if (e.code === 'Enter' || e.code === 'NumpadEnter') {
      e.preventDefault();
      submitBranch();
    } else if (e.code === 'Escape') {
      e.preventDefault();
      cancelBranch();
    }
  }

  /** Stash messages, and merge subjects when muted merges are on, use the secondary text color. */
  function isMuted(r: GraphRow): boolean {
    return r.kind === 'stash' || (settings.showMergesMuted && r.kind === 'commit' && r.parents.length > 1);
  }

  /** Color class of a row's ref connector: its lane's color; stash rows take their dashed line's. */
  // Relative dates ("5m") would freeze in rows that stay rendered: re-render them once a minute.
  let minute = $state(0);
  $effect(() => {
    if (settings.dateFormat !== 'relative') return;
    const timer = setInterval(() => minute++, 60_000);
    return () => clearInterval(timer);
  });

  function dateText(time: number): string {
    void minute;
    return formatTime(time, settings.dateFormat);
  }

  // ───────────── selection ─────────────

  let navSeq = 0;
  /** Target of a keyboard move whose row is still loading (repeated keys continue from it). */
  let navTarget: number | null = null;

  /** Select row i (clamped). Later calls supersede earlier ones that are still loading their row. */
  async function select(i: number, align: 'auto' | 'center' = 'auto'): Promise<void> {
    const n = graph.total;
    if (n === 0) return;
    const row = Math.max(0, Math.min(n - 1, i));
    const seq = ++navSeq;
    navTarget = row;
    await graph.getRow(row);
    if (seq !== navSeq) return;
    navTarget = null;
    await repo.selectRow(row, align);
  }

  /** The first fully visible row. The list's own mapping: a huge history's physical scrollTop is paged. */
  function firstVisible(): number {
    return list ? Math.min(graph.total - 1, list.firstVisibleIndex()) : 0;
  }

  function move(delta: number): void {
    const base = navTarget ?? graph.selectedRow;
    void select(base == null ? firstVisible() : base + delta);
  }

  /** ⌥↓: the first parent of the selection (or of the row a pending move is heading to). */
  async function selectParent(): Promise<void> {
    const i = navTarget ?? graph.selectedRow;
    if (i == null) return;
    // Any key handled meanwhile (it bumps navSeq) wins over this lookup.
    const seq = navSeq;
    const parent = (await graph.getRow(i))?.parents[0];
    if (!parent || seq !== navSeq) return;
    const j = await graph.find(parent);
    if (seq !== navSeq) return;
    if (j != null) {
      void select(j);
    } else {
      navSeq++;
      navTarget = null;
      void repo.reveal(parent);
    }
  }

  /**
   * ⌥↑: the nearest row above whose first parent is the selection (else any parent), within CHILD_SCAN
   * rows. Rows are read from the current generation only: a placeholder row left from before a reload
   * may sit at another index than its commit has now.
   */
  async function selectChild(): Promise<void> {
    const i = navTarget ?? graph.selectedRow;
    if (i == null) return;
    // Any key handled meanwhile (it bumps navSeq) or a reload wins over this lookup.
    const seq = navSeq;
    const gen = graph.generation;
    const superseded = () => seq !== navSeq || graph.generation !== gen;
    const read = rowReader(graph, gen);
    const oid = (await read(i))?.oid; // '' (WIP) has no children
    if (!oid || superseded()) return;
    let any: number | null = null;
    for (let k = i - 1; k >= 0 && k >= i - CHILD_SCAN; k--) {
      const c = await read(k);
      if (superseded()) return;
      if (!c) continue;
      if (c.parents[0] === oid) {
        void select(k);
        return;
      }
      if (any == null && c.parents.includes(oid)) any = k;
    }
    if (any != null) void select(any);
  }

  /** Whether row i can be compared with the selection (a commit, stash, compare or this worktree's WIP). */
  function canCompareWith(i: number): boolean {
    const sel = repo.selection;
    const cur = graph.selectedRow;
    if (cur == null || cur === i) return false;
    return (
      sel.kind === 'commit' ||
      sel.kind === 'stash' ||
      sel.kind === 'compare' ||
      (sel.kind === 'wip' && sel.cwd === repo.id)
    );
  }

  /** ⌘-press: compare row i with the selection; plain selection when there is nothing to compare with. */
  function compareWith(i: number): void {
    if (!canCompareWith(i)) {
      void select(i);
      return;
    }
    navSeq++;
    navTarget = null;
    void repo.compareWithRow(i);
  }

  /** ⇧-press: the commits from the range anchor (else the selection) to row i. */
  function rangeTo(i: number): void {
    navSeq++;
    navTarget = null;
    void repo.selectRange(i);
  }

  /** ⇧↑ / ⇧↓: move the free end of the range (starting one at the selection) by a commit. */
  function extendRange(dir: 1 | -1): void {
    navSeq++;
    navTarget = null;
    void repo.extendRange(dir);
  }

  /** Back from compare to the primary selection. */
  function leaveCompare(): boolean {
    const sel = repo.selection;
    if (sel.kind !== 'compare') return false;
    const i = graph.selectedRow;
    if (i != null) void select(i);
    else if (sel.to === '') {
      // The working tree end ('') is no oid: back to the WIP selection.
      graph.compareRow = null;
      repo.select({ kind: 'wip', cwd: repo.id });
    } else void repo.reveal(sel.to);
    return true;
  }

  /** The full sha ⌘C copies: none for the working tree (the WIP row and a compare's '' end). */
  function selectedOid(): string | null {
    const sel = repo.selection;
    if (sel.kind === 'commit' || sel.kind === 'stash') return sel.oid;
    if (sel.kind === 'compare') {
      const i = graph.selectedRow;
      const r = i != null && graph.isFresh(i) ? graph.row(i) : undefined;
      return r?.oid || sel.to || null;
    }
    return null;
  }

  // ───────────── opening diffs ─────────────

  let openSeq = 0;

  function fileRef(f: FileChange | StatusEntry): FileRef {
    return { path: f.path, oldPath: f.oldPath, status: f.status };
  }

  function itemsOf(target: DiffTarget, files: (FileChange | StatusEntry)[]): DiffItem[] {
    return files.map((f) => ({ target, file: fileRef(f) }));
  }

  /** Working-tree files of the worktree at `cwd`: conflicted, staged, unstaged, untracked. */
  function wipItems(cwd: string, s: WorkingStatus | null): DiffItem[] {
    if (!s) return [];
    return [
      ...itemsOf({ kind: 'conflicted', cwd }, s.conflicted),
      ...itemsOf({ kind: 'staged', cwd }, s.staged),
      ...itemsOf({ kind: 'unstaged', cwd }, s.unstaged),
      ...itemsOf({ kind: 'untracked', cwd }, s.untracked),
    ];
  }

  /** A stash's files, then the untracked files it saved (when it has an untracked commit). */
  async function stashItems(oid: string): Promise<DiffItem[]> {
    const target: DiffTarget = { kind: 'stash', oid };
    const untracked: DiffTarget = { kind: 'stashUntracked', oid };
    const withUntracked = !!repo.refs?.stashes.find((s) => s.oid === oid)?.untrackedCommit;
    const [files, extra] = await Promise.all([
      getDiffFiles(repo.id, target),
      withUntracked ? getDiffFiles(repo.id, untracked) : Promise.resolve([]),
    ]);
    return [...itemsOf(target, files), ...itemsOf(untracked, extra)];
  }

  async function commitItems(oid: string): Promise<DiffItem[]> {
    const details = await getCommitDetails(repo.id, oid);
    return itemsOf({ kind: 'commit', oid, parent: null }, details.files);
  }

  /** Every changed file of a graph row, in display order. */
  async function rowItems(r: GraphRow): Promise<DiffItem[]> {
    if (r.kind === 'wip') return wipItems(repo.id, repo.status);
    if (r.kind === 'stash') return stashItems(r.oid);
    return commitItems(r.oid);
  }

  /** Every changed file of the selection, also when it has no graph row (a revealed commit outside the
   *  loaded history, another worktree's changes picked in the sidebar). */
  async function selectionItems(): Promise<DiffItem[]> {
    const sel = repo.selection;
    switch (sel.kind) {
      case 'commit':
        return commitItems(sel.oid);
      case 'stash':
        return stashItems(sel.oid);
      case 'wip':
        return wipItems(sel.cwd, sel.cwd === repo.id ? repo.status : await api.status(sel.cwd));
      case 'compare': {
        if (sel.to !== '') {
          const target: DiffTarget = { kind: 'range', from: sel.from, to: sel.to };
          return itemsOf(target, await getDiffFiles(repo.id, target));
        }
        // A commit vs the working tree: its tracked changes, then the untracked files.
        const target: DiffTarget = { kind: 'worktree', cwd: repo.id, oid: sel.from };
        const files = await getDiffFiles(repo.id, target);
        const untracked = repo.status?.untracked ?? [];
        return [...itemsOf(target, files), ...itemsOf({ kind: 'untracked', cwd: repo.id }, untracked)];
      }
      default:
        return [];
    }
  }

  /** Open the first of the loaded files, unless the user moved on meanwhile (`still()` false). */
  async function openItems(load: () => Promise<DiffItem[]>, still: () => boolean): Promise<void> {
    const seq = ++openSeq;
    try {
      const items = await load();
      if (seq === openSeq && still() && repo.view.kind === 'graph' && items.length) repo.openDiff(items, 0);
    } catch (e) {
      if (seq === openSeq) toast(errorMessage(e));
    }
  }

  /** Open the first changed file of row i in the diff viewer. */
  function openRow(i: number): void {
    void openItems(
      async () => {
        const r = await graph.getRow(i);
        return r ? rowItems(r) : [];
      },
      () => graph.selectedRow === i || navTarget === i,
    );
  }

  /** → / Enter: the first file of the selection (of the row a pending move is heading to). */
  function openSelected(): void {
    if (navTarget != null) return openRow(navTarget);
    const sel = repo.selection;
    void openItems(selectionItems, () => repo.selection === sel);
  }

  // ───────────── search ─────────────

  /** A finished search whose current match was not selected: the user had moved on while it ran. */
  let skipped: SearchState | null = null;

  function openSearch(): void {
    searchOpen = true;
    searchFocus++;
  }

  function closeSearch(): void {
    searchOpen = false;
    skipped = null;
    graph.clearSearch();
    list?.focus();
  }

  function step(dir: 1 | -1): void {
    const st = graph.search;
    // While a search runs, `rows` still holds the previous query's matches.
    if (!st || st.busy) return;
    // After a skipped jump the next match is the current one (the first at/after the selection when the
    // search finished), not the one after it. A step or a re-run replaces `st`, which ends this.
    const cur = graph.currentMatch;
    if (dir === 1 && st === skipped && cur != null && cur !== graph.selectedRow) {
      skipped = null;
      void select(cur, 'center');
      return;
    }
    const row = graph.stepMatch(dir);
    if (row != null) void select(row, 'center');
  }

  function onMatch(row: number, jump: boolean): void {
    if (jump) void select(row, 'center');
    else skipped = graph.search;
  }

  // ───────────── input ─────────────

  function rowAt(e: Event): number | null {
    const el = e.target instanceof Element ? e.target.closest<HTMLElement>('[data-row]') : null;
    if (!el) return null;
    const i = Number(el.dataset.row);
    return Number.isInteger(i) && i >= 0 && i < graph.total ? i : null;
  }

  function onMouseDown(e: MouseEvent): void {
    // Ctrl-click is the macOS context click.
    if (e.button !== 0 || e.ctrlKey) return;
    // The branch-name input keeps the focus (anywhere else: the list takes it and the input's blur
    // closes the prompt).
    if (e.target instanceof Element && e.target.closest('.binput')) return;
    list?.focus();
    const i = rowAt(e);
    if (i == null) return;
    // A text selection left in another pane must not capture ⌘C once a row is picked.
    window.getSelection()?.removeAllRanges();
    if (e.metaKey) compareWith(i);
    else if (e.shiftKey) rangeTo(i);
    else void select(i);
  }

  function onDblClick(e: MouseEvent): void {
    if (e.metaKey || e.ctrlKey) return;
    const i = rowAt(e);
    if (i != null) openRow(i);
  }

  function copy(text: string): void {
    void copyText(text).then(() => toast('Copied'));
  }

  /**
   * Copy text that is still loading. The ClipboardItem takes the promise, so the write starts inside
   * the menu click's user gesture: WKWebView refuses a clipboard write made after an await.
   */
  function copyLater(text: Promise<string>): void {
    const blob = text.then((t) => new Blob([t], { type: 'text/plain' }));
    navigator.clipboard.write([new ClipboardItem({ 'text/plain': blob })]).then(
      () => toast('Copied'),
      (err: unknown) => toast(errorMessage(err)),
    );
  }

  /** Full commit message: subject, blank line, body. */
  async function messageOf(oid: string): Promise<string> {
    const d = await getCommitDetails(repo.id, oid);
    return d.body ? `${d.subject}\n\n${d.body}` : d.subject;
  }

  function onContextMenu(e: MouseEvent): void {
    const i = rowAt(e);
    const r = i == null ? undefined : graph.row(i);
    if (i == null || !r) {
      if (e.target instanceof Element && e.target.closest('[data-row]')) e.preventDefault();
      else columnMenu(e);
      return;
    }
    if (r.kind === 'wip') {
      showContextMenu(e, [
        {
          label: 'Stash changes',
          icon: Archive,
          disabled: !repo.dirty || !!repo.ops.busy,
          action: () => void repo.ops.stash(),
        },
      ]);
      return;
    }
    const cur = graph.selectedRow;
    const canCompare = canCompareWith(i);
    const stash = r.kind === 'stash' ? repo.refs?.stashes.find((s) => s.oid === r.oid) : undefined;
    const items: MenuEntry[] = [
      { label: 'Copy SHA', icon: Copy, hint: i === cur ? '⌘C' : undefined, action: () => copy(r.oid) },
      { label: 'Copy short SHA', icon: Hash, action: () => copy(shortOid(r.oid)) },
      { label: 'Copy subject', icon: TextQuote, action: () => copy(r.subject) },
      { label: 'Copy message', icon: MessageSquareText, action: () => copyLater(messageOf(r.oid)) },
      { label: 'Copy author', icon: User, action: () => copy(r.email ? `${r.author} <${r.email}>` : r.author) },
      { separator: true },
      { label: 'Compare with selected', icon: GitCompare, disabled: !canCompare, action: () => compareWith(i) },
      {
        label: 'Browse files',
        icon: FolderTree,
        action: () => repo.openView({ kind: 'tree', rev: r.oid, path: null }),
      },
    ];
    if (r.kind === 'commit') {
      items.push(
        { separator: true },
        { label: 'Create branch here', icon: GitBranchPlus, action: () => repo.promptBranch(i, r.oid, false) },
      );
    } else if (stash) {
      const busy = !!repo.ops.busy;
      items.push(
        { separator: true },
        { label: 'Apply', icon: Archive, disabled: busy, action: () => void repo.ops.apply(stash) },
        { label: 'Pop', icon: ArchiveArrowUp, disabled: busy, action: () => void repo.ops.pop(stash) },
        { label: 'Drop…', icon: ArchiveX, disabled: busy, action: () => void repo.ops.drop(stash) },
      );
    }
    showContextMenu(e, items);
  }

  /** Right-click on the header, below the rows or on a column handle: column toggles. */
  function columnMenu(e: MouseEvent): void {
    showContextMenu(
      e,
      COLUMN_TOGGLES.map(
        (t): MenuEntry => ({
          label: t.label,
          icon: settings[t.key] ? Check : undefined,
          action: () => {
            settings[t.key] = !settings[t.key];
          },
        }),
      ),
    );
  }

  function onKey(e: KeyboardEvent): void {
    // A viewer covering the graph makes it inert, but WebKit may leave the focus on the list.
    if (isComposing(e) || contextMenu.open || !isActive()) return;
    const none = !e.metaKey && !e.ctrlKey && !e.altKey;
    const shift = none && e.shiftKey;
    const cmd = e.metaKey && !e.ctrlKey && !e.altKey;
    const alt = e.altKey && !e.metaKey && !e.ctrlKey;
    const page = list?.pageSize() ?? 1;
    let handled = true;
    switch (e.code) {
      case 'ArrowDown':
        if (shift) extendRange(1);
        else if (none) move(1);
        else if (cmd) void select(graph.total - 1);
        else if (alt) void selectParent();
        else handled = false;
        break;
      case 'ArrowUp':
        if (shift) extendRange(-1);
        else if (none) move(-1);
        else if (cmd) void select(0);
        else if (alt) void selectChild();
        else handled = false;
        break;
      case 'KeyJ':
        if (none && !e.shiftKey) move(1);
        else handled = false;
        break;
      case 'KeyK':
        if (none && !e.shiftKey) move(-1);
        else handled = false;
        break;
      case 'PageDown':
        if (none) move(page);
        else handled = false;
        break;
      case 'PageUp':
        if (none) move(-page);
        else handled = false;
        break;
      case 'Home':
        if (none) void select(0);
        else handled = false;
        break;
      case 'End':
        if (none) void select(graph.total - 1);
        else handled = false;
        break;
      case 'ArrowRight':
      case 'Enter':
      case 'NumpadEnter':
        if (none) openSelected();
        else handled = false;
        break;
      case 'Escape':
        if (none && repo.branchPrompt) cancelBranch();
        else if (none && searchOpen) closeSearch();
        else handled = none && leaveCompare();
        break;
      default:
        handled = false;
    }
    if (handled) {
      e.preventDefault();
      e.stopPropagation();
    }
  }

  /**
   * While the find box is open in the active tab: ⌘G / ⌘⇧G step through matches from anywhere, and
   * an Esc no focused owner took (focus on the page body) closes the box.
   */
  function onWindowKey(e: KeyboardEvent): void {
    if (e.defaultPrevented || isComposing(e) || !searchOpen || !isActive()) return;
    const inSearch = e.target instanceof Element && !!e.target.closest('.gsearch');
    if (isEditable(e.target) && !inSearch) return;
    if (e.code === 'KeyG' && e.metaKey && !e.ctrlKey && !e.altKey) {
      e.preventDefault();
      step(e.shiftKey ? -1 : 1);
    } else if (e.code === 'Escape' && !e.metaKey && !e.ctrlKey && !e.altKey && !contextMenu.open) {
      e.preventDefault();
      closeSearch();
    }
  }

  /** What ⌘C copies: the selected full sha while the list has focus and no text is selected. */
  function copyTarget(): string | null {
    const vp = list?.getViewport();
    if (!vp || document.activeElement !== vp || !isActive()) return null;
    const textSel = window.getSelection();
    if (textSel && !textSel.isCollapsed && textSel.toString()) return null;
    return selectedOid();
  }

  // ⌘C is the native Edit → Copy accelerator: it only arrives as DOM clipboard events. WebKit keeps
  // Copy disabled without a text selection unless `beforecopy` is cancelled.
  $effect(() => {
    const onBeforeCopy = (e: Event) => {
      if (copyTarget()) e.preventDefault();
    };
    const onCopy = (e: ClipboardEvent) => {
      const oid = copyTarget();
      if (!oid || !e.clipboardData) return;
      e.clipboardData.setData('text/plain', oid);
      e.preventDefault();
      toast('Copied');
    };
    document.addEventListener('beforecopy', onBeforeCopy);
    document.addEventListener('copy', onCopy);
    return () => {
      document.removeEventListener('beforecopy', onBeforeCopy);
      document.removeEventListener('copy', onCopy);
    };
  });

  // ───────────── wheel outside the list ─────────────

  /** Svelte action: a passive wheel listener (an `onwheel` attribute registers a blocking one). */
  function passiveWheel(node: HTMLElement, handler: (e: WheelEvent) => void) {
    node.addEventListener('wheel', handler, { passive: true });
    return {
      destroy() {
        node.removeEventListener('wheel', handler);
      },
    };
  }

  /**
   * The column header and the column handles lie over the rows but outside the list's scroller, so a
   * wheel over them scrolls nothing (WebKit picks the scroller from the layer under the pointer): scroll
   * the list by the deltas instead, as the sidebar does under its pinned header.
   */
  function forwardWheel(e: WheelEvent): void {
    const vp = list?.getViewport();
    if (!list || !vp) return;
    const unit =
      e.deltaMode === WheelEvent.DOM_DELTA_LINE
        ? ROW_H
        : e.deltaMode === WheelEvent.DOM_DELTA_PAGE
          ? list.pageSize() * ROW_H
          : 1;
    if (e.deltaY) list.setScrollTop(list.getScrollTop() + e.deltaY * unit);
    if (e.deltaX) vp.scrollLeft += e.deltaX * unit;
  }

  // ───────────── column resize ─────────────

  function clamp(v: number, lo: number, hi: number): number {
    return Math.max(lo, Math.min(hi, v));
  }

  interface Drag {
    key: ColKey;
    pointerId: number;
    x0: number;
    x: number;
    /** Column width when the drag started. */
    w0: number;
    cap0: number;
    natural: number;
  }

  let drag: Drag | null = null;
  let dragFrame = 0;

  $effect(() => () => cancelAnimationFrame(dragFrame));

  function applyResize(): void {
    dragFrame = 0;
    const d = drag;
    if (!d) return;
    const dx = d.x - d.x0;
    if (d.key === 'colGraph') {
      // The column never exceeds the lanes' width; dragging to (or past) it lifts the cap.
      const w = Math.round(clamp(d.w0 + dx, MIN_GRAPH_W, d.natural));
      settings.colGraph = w < d.natural ? w : Math.max(d.cap0, d.natural);
    } else if (d.key === 'colRefs') {
      // Left-anchored like the graph: grows when its right edge moves right.
      settings.colRefs = Math.round(clamp(d.w0 + dx, MIN_REFS_W, MAX_REFS_W));
    } else {
      // Right-anchored columns grow when their left edge moves left.
      const [lo, hi] = COL_LIMITS[d.key];
      settings[d.key] = Math.round(clamp(d.w0 - dx, lo, hi));
    }
  }

  function onHandleDown(e: PointerEvent, key: ColKey): void {
    // Ctrl-click is the macOS context click (the column menu).
    if (e.button !== 0 || e.ctrlKey) return;
    if (e.currentTarget instanceof HTMLElement) e.currentTarget.setPointerCapture(e.pointerId);
    drag = {
      key,
      pointerId: e.pointerId,
      x0: e.clientX,
      x: e.clientX,
      w0: key === 'colGraph' ? graphW : key === 'colRefs' ? refsW : settings[key],
      cap0: settings.colGraph,
      natural: naturalWidth(maxLanes),
    };
    resizing = key;
  }

  function onHandleMove(e: PointerEvent): void {
    if (!drag || e.pointerId !== drag.pointerId) return;
    drag.x = e.clientX;
    if (!dragFrame) dragFrame = requestAnimationFrame(applyResize);
  }

  function onHandleUp(e: PointerEvent): void {
    if (!drag || e.pointerId !== drag.pointerId) return;
    if (dragFrame) {
      cancelAnimationFrame(dragFrame);
      if (e.type === 'pointerup') drag.x = e.clientX;
      applyResize();
    }
    drag = null;
    resizing = null;
  }

  function resetColumn(key: ColKey): void {
    settings[key] = DEFAULT_SETTINGS[key];
  }
</script>

<svelte:window onkeydown={onWindowKey} />

<div
  class="gv"
  class:gv-focused={listFocused}
  class:resizing={resizing != null}
  style:--rw="{refsW}px"
  style:--gw="{graphW}px"
  style:--cols={cols}
>
  <!-- Column header over the list's width (a scrollbar takes none of it): the names of the branch / tag,
       graph and subject columns scroll sideways with the rows; those of the columns right of the subject
       sit at the right edge like their sticky cells. The gear at the far right opens the column picker. -->
  <div class="colhead" role="presentation" oncontextmenu={columnMenu} use:passiveWheel={forwardWheel}>
    <div class="hwrap" style:width={listWidth > 0 ? `${listWidth}px` : null}>
      <div
        class="cols"
        style:grid-template-columns={headCols}
        style:transform={shift ? `translateX(${-shift}px)` : null}
      >
        {#if refsW}
          <span class="h h-refs">Branch / Tag</span>
        {/if}
        <span class="h h-graph">Graph</span>
        <span class="h h-subj">Message</span>
      </div>
      {#if layout.right}
        <div class="rcols" style:grid-template-columns={rightCols}>
          {#if layout.compactAuthor}
            <span class="h ic" title="Author"><User size={13} strokeWidth={1.75} /></span>
          {:else if layout.author}
            <span class="h">Author</span>
          {/if}
          {#if layout.date}
            <span class="h">Date</span>
          {:else if layout.time}
            <span class="h ic" title="Date"><Clock size={13} strokeWidth={1.75} /></span>
          {/if}
          {#if layout.sha}
            <span class="h">SHA</span>
          {/if}
        </div>
      {/if}
    </div>
    <div class="gearbox">
      <button
        class="icon-btn"
        class:on={pickerOpen}
        bind:this={gear}
        title="Columns"
        aria-expanded={pickerOpen}
        onmousedown={(e) => e.preventDefault()}
        onclick={() => (pickerOpen = !pickerOpen)}
      >
        <Columns3Cog size={14} strokeWidth={1.75} />
      </button>
    </div>
  </div>

  <div
    class="body"
    role="presentation"
    onmousedown={onMouseDown}
    ondblclick={onDblClick}
    oncontextmenu={onContextMenu}
    onfocusin={() => (listFocused = true)}
    onfocusout={() => (listFocused = false)}
  >
    <VirtualList
      bind:this={list}
      {count}
      rowHeight={ROW_H}
      overscan={OVERSCAN}
      tabindex={0}
      horizontal
      contentWidth={contentW}
      onrange={(start, end) => (range = { start, end })}
      onkeydown={onKey}
    >
      {#snippet overlay(start: number, end: number)}
        <GraphLanes {start} {end} width={graphW} left={refsW} />
        {#if layout.time}
          <TimeMarks {start} {end} left={refsW + graphW + layout.subject + layout.author + 6} tick={minute} />
        {/if}
      {/snippet}
      {#snippet row(i: number)}
        {@const r = graph.row(i)}
        {#if r}
          {@const prompting = repo.branchPrompt?.row === i && r.kind === 'commit'}
          <!-- The row's highlight band starts at its node (bandStart, the row's nx variable); the columns left of it stay plain. -->
          <div
            class="row"
            class:sel={i === graph.selectedRow}
            class:cmp={i === graph.compareRow}
            class:match={graph.matchSet.has(i)}
            class:cur={i === currentMatch}
            data-row={i}
            style:--nx="{bandStart(r.lane, refsW, graphW)}px"
          >
            {#if refsW && r.kind !== 'wip' && (prompting || r.refs.length)}
              <div class="refcell" style:width="{refsCellWidth(r.lane, refsW, graphW)}px">
                {#if prompting}
                  {@render branchInput()}
                {:else}
                  <div class="labels">
                    <RefBadges refs={r.refs} worktrees={elsewhere} max={refsMax} column />
                  </div>
                {/if}
                <span class="link"></span>
              </div>
            {/if}
            <!-- Time marks straddle the next row's top edge; WIP stays left of their slot. -->
            <div
              class="subj"
              class:dim={isMuted(r)}
              class:wip={r.kind === 'wip'}
              style:max-width={r.kind === 'wip' && layout.time ? `${layout.subject + layout.author}px` : null}
            >
              {#if r.kind === 'wip'}
                <span class="wip-label" title="Working tree"><span class="ellipsis">// WIP</span></span>
                <span class="wip-counts">
                  {#if wip.changed}
                    <span class="wc" title="Changed tracked files">
                      <span class="st st-M"><StatusIcon status="M" /></span>{wip.changed}
                    </span>
                  {/if}
                  {#if wip.added}
                    <span class="wc" title="Added / untracked files">
                      <span class="st st-A"><StatusIcon status="A" /></span>{wip.added}
                    </span>
                  {/if}
                  {#if wip.deleted}
                    <span class="wc" title="Deleted files">
                      <span class="st st-D"><StatusIcon status="D" /></span>{wip.deleted}
                    </span>
                  {/if}
                  {#if wip.conflicts}
                    <span class="wc" title="Conflicted">
                      <span class="st st-U"><StatusIcon status="U" /></span>{wip.conflicts}
                    </span>
                  {/if}
                </span>
              {:else}
                {#if prompting && !refsW}
                  {@render branchInput()}
                {:else if r.refs.length && !refsW}
                  <RefBadges refs={r.refs} worktrees={elsewhere} />
                {/if}
                <span class="text ellipsis">{r.subject}</span>
              {/if}
            </div>
            {#if r.kind !== 'wip'}
              {#if layout.compactAuthor}
                <div class="author av sticky" style:right="{stickAuthor}px">
                  {#if r.author}
                    <Avatar name={r.author} email={r.email} size={16} />
                  {/if}
                </div>
              {:else if layout.author}
                <div class="author ellipsis sticky" style:right="{stickAuthor}px">{r.author}</div>
              {/if}
              {#if layout.date}
                <div
                  class="date ellipsis sticky"
                  style:right="{stickDate}px"
                  title={r.time ? absoluteTime(r.time) : undefined}
                >
                  {dateText(r.time)}
                </div>
              {:else if layout.time}
                <!-- the time slot: its marks float in the overlay -->
                <div class="sticky" style:right="{stickDate}px"></div>
              {/if}
              {#if layout.sha}
                <div class="sha mono ellipsis sticky" style:right="0px">{shortOid(r.oid)}</div>
              {/if}
            {/if}
          </div>
        {:else if i < total}
          <!-- Unloaded: the lane is unknown, so the band starts at the graph column (the --nx default). -->
          <div class="row" class:sel={i === graph.selectedRow} class:cmp={i === graph.compareRow} data-row={i}>
            <div class="subj"><span class="ph"></span></div>
            {#if layout.right}
              <div class="sticky" style:grid-column="2 / -1" style:right="0px"></div>
            {/if}
          </div>
        {:else}
          <div class="row trunc" title="History truncated">
            <ChevronsDown size={14} strokeWidth={1.75} />
          </div>
        {/if}
      {/snippet}
    </VirtualList>
    {#if total > 0 && scrollable}
      <ScrollMarkers {total} />
    {/if}
  </div>

  {#if pickerOpen}
    <ColumnPicker anchor={gear} onclose={() => (pickerOpen = false)} />
  {/if}

  {#if total > 0}
    {#if listWidth > 0}
      {#each handles as h (h.key)}
        <!-- Pointer-only column handle; mousedown is cancelled so a drag never moves keyboard focus.
             Right-click: the column toggles, also when the rows leave no room below them. A wheel
             scrolls the list, which the handle covers without being part of it. -->
        <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
        <div
          class="handle"
          class:on={resizing === h.key}
          style:left="{h.x - 3}px"
          role="separator"
          aria-orientation="vertical"
          onpointerdown={(e) => onHandleDown(e, h.key)}
          onpointermove={onHandleMove}
          onpointerup={onHandleUp}
          onpointercancel={onHandleUp}
          onlostpointercapture={onHandleUp}
          onmousedown={(e) => e.preventDefault()}
          ondblclick={() => resetColumn(h.key)}
          oncontextmenu={columnMenu}
          use:passiveWheel={forwardWheel}
        ></div>
      {/each}
    {/if}
  {/if}

  {#if filtered}
    <button class="chip" title={filterTitle} onclick={() => repo.clearFilters()}>
      <Funnel size={12} strokeWidth={1.75} />
    </button>
  {/if}

  {#snippet branchInput()}
    <!-- The branch-name prompt (repo.branchPrompt) in the commit's row: Enter creates, Esc / blur cancels. -->
    <input
      class="binput"
      placeholder="branch name"
      spellcheck="false"
      autocomplete="off"
      autocorrect="off"
      autocapitalize="off"
      aria-label="New branch name"
      bind:value={branchName}
      use:autofocus
      onkeydown={onBranchKey}
      onblur={cancelBranch}
    />
  {/snippet}

  {#if searchOpen}
    <GraphSearch
      bind:text={searchText}
      bind:mode={searchMode}
      focusSeq={searchFocus}
      onstep={step}
      onmatch={onMatch}
      onclose={closeSearch}
    />
  {/if}
</div>

<style>
  .gv {
    position: relative;
    flex: 1 1 auto;
    width: 100%;
    height: 100%;
    min-height: 0;
    overflow: hidden;
    background: var(--bg-graph);
  }
  .gv.resizing,
  .gv.resizing :global(*) {
    cursor: col-resize;
  }
  /* Below the handles (z 3), so a drag may start on the header too. Header classes (colhead, h-*) must
     not reuse row cell class names (a shared `head` once styled the HEAD row's subject as the header). */
  .colhead {
    position: absolute;
    top: 0;
    left: 0;
    right: 0;
    z-index: 2;
    height: var(--header-h);
    border-bottom: 1px solid var(--border);
    background: var(--bg-1);
  }
  .hwrap {
    position: relative;
    width: 100%;
    height: 100%;
    overflow: hidden;
  }
  .cols,
  .rcols {
    display: grid;
    align-items: center;
    height: 100%;
  }
  /* The right-hand names cover the scrolled ones, as the sticky cells cover the subject. */
  .rcols {
    position: absolute;
    top: 0;
    right: 0;
    background: var(--bg-1);
  }
  .h {
    min-width: 0;
    padding-right: 10px;
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
    font-size: var(--fs-colhead);
    font-weight: 400;
    letter-spacing: 0.08em;
    text-transform: uppercase;
    color: var(--colhead-fg);
  }
  /* Graph: at lane 0's node edge (geometry PAD), the same inset as the branch / tag labels. */
  .h-refs,
  .h-graph {
    padding-left: 8px;
  }
  .h-subj {
    padding-left: 4px;
  }
  /* Icon names (avatar column, time slot) start where their cells' content does. */
  .h.ic {
    display: flex;
    align-items: center;
    padding: 0 0 0 6px;
  }
  /* The last name keeps clear of the gear (an icon name is covered instead). */
  .rcols > .h:last-child:not(.ic) {
    padding-right: 28px;
  }
  /* Opaque, so a column name running under the gear is cut rather than showing through it. */
  .gearbox {
    position: absolute;
    top: 0;
    right: 0;
    bottom: 0;
    display: flex;
    align-items: center;
    padding: 0 2px;
    background: var(--bg-1);
  }
  .gearbox .icon-btn {
    width: 18px;
    height: 18px;
  }

  .body {
    position: absolute;
    top: calc(var(--header-h) + var(--sp-2));
    right: 0;
    bottom: 0;
    left: 0;
  }

  /* The row's state paints a band from its node's center (--nx, set per row; the graph column's left
     edge for an unloaded row) to the right edge: the branch / tag column and the lanes left of the node
     stay on the graph background, as in GitKraken. --row-bg is the band's (opaque) color, which the
     sticky cells paint too. */
  .row {
    --row-bg: var(--bg-graph);
    --nx: var(--rw);
    position: relative;
    display: grid;
    grid-template-columns: var(--cols);
    align-items: center;
    height: 100%;
    padding-left: calc(var(--rw) + var(--gw));
    font-size: var(--fs-sm);
    color: var(--text-1);
    background: linear-gradient(to right, var(--bg-graph) var(--nx), var(--row-bg) var(--nx));
  }
  .row:hover {
    --row-bg: var(--bg-hover);
  }
  .row.match {
    --row-bg: var(--bg-match);
  }
  .row.cur {
    --row-bg: var(--bg-match-current);
  }
  .row.sel,
  .row.cmp {
    --row-bg: var(--bg-active);
  }
  /* The compare row's mark: a 2px ink bar at the message column's left edge, inside the band. */
  .row.cmp {
    background: linear-gradient(
      to right,
      var(--bg-graph) var(--nx),
      var(--row-bg) var(--nx) calc(var(--rw) + var(--gw)),
      var(--accent) calc(var(--rw) + var(--gw)) calc(var(--rw) + var(--gw) + 2px),
      var(--row-bg) calc(var(--rw) + var(--gw) + 2px)
    );
  }
  .gv-focused .row.sel {
    --row-bg: var(--bg-active-strong);
  }

  /* Cells right of the subject stick to the list's right edge while the rows scroll sideways (each one
     offset by the cells after it). They fill the row's height in the row's band color, so the subject
     sliding under them is covered; above the lanes SVG (z 1), below the time marks (z 3). */
  .sticky {
    position: sticky;
    z-index: 2;
    align-self: stretch;
    line-height: var(--row-h);
    background: var(--row-bg);
  }

  /* Branch / tag cell: from the row's left edge to just short of its node, the labels then a faint
     line at the node's height (the lanes SVG above the rows paints over it where lanes cross). */
  .refcell {
    position: absolute;
    top: 0;
    left: 0;
    display: flex;
    align-items: center;
    height: 100%;
  }
  /* The labels end at least 12px short of the graph column, never in it. */
  .labels {
    display: flex;
    flex: 0 1 auto;
    min-width: 0;
    max-width: calc(var(--rw) - 12px);
    padding-left: 8px;
  }
  /* Solid, a hair above the row's background in every row state (the token is translucent ink). */
  .link {
    flex: 1 1 0;
    min-width: 6px;
    height: 1px;
    margin-left: 4px;
    background: var(--ref-link);
  }
  /* The branch-name prompt: a label-sized field in the branch / tag cell (inline before the subject with
     the column off), bounded like the labels. */
  .binput {
    flex: 1 1 auto;
    min-width: 48px;
    max-width: calc(var(--rw) - 12px);
    height: 20px;
    margin-left: 8px;
    padding: 0 6px;
    border: 1px solid var(--focus-ring);
    border-radius: var(--radius-xs);
    background: var(--bg-input);
    color: var(--text-0);
    font-size: var(--fs-sm);
    outline: none;
  }
  .binput::placeholder {
    color: var(--text-2);
  }
  .subj .binput {
    flex: 0 1 180px;
    max-width: 180px;
    margin-left: 0;
  }

  /* Subject a shade under primary ink, author muted (the date and sha stay muted); the selected and the
     compare row lift both a tier. A muted subject (stash, merge) lifts to the plain rows' shade. */
  .subj {
    display: flex;
    align-items: center;
    gap: 6px;
    min-width: 0;
    height: 100%;
    padding: 0 10px 0 4px;
    overflow: hidden;
    font-size: var(--fs);
    color: var(--row-fg);
  }
  .subj.dim {
    color: var(--text-1);
  }
  .row.sel .subj,
  .row.cmp .subj {
    color: var(--row-fg-sel);
  }
  .row.sel .subj.dim,
  .row.cmp .subj.dim {
    color: var(--row-fg);
  }
  .author {
    color: var(--row-author-fg);
  }
  .row.sel .author,
  .row.cmp .author {
    color: var(--row-author-fg-sel);
  }
  /* Basis 0: the subject gets what the badges leave, so it truncates before they do. */
  .text {
    flex: 1 1 0;
  }
  .author,
  .date,
  .sha {
    padding-right: 10px;
  }
  /* Avatar-only author column (AVATAR_COL_W): the avatar centered, the name in its title. */
  .author.av {
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 0;
  }
  .date,
  .sha {
    color: var(--text-2);
    font-variant-numeric: tabular-nums;
  }

  .subj.wip {
    grid-column: 1 / -1;
    gap: var(--sp-3);
  }
  .wip-label {
    flex: 0 1 var(--wip-label-w);
    min-width: 0;
    height: var(--wip-label-h);
    padding: 0 var(--sp-2);
    display: inline-flex;
    align-items: center;
    border: var(--wip-label-border-w) solid var(--border);
    border-radius: var(--radius-sm);
    background: var(--bg-graph);
    color: var(--text-2);
    font-style: italic;
    font-weight: 600;
    white-space: nowrap;
  }
  .wip-counts {
    display: inline-flex;
    align-items: center;
    gap: var(--sp-2);
    flex: none;
  }
  .wc {
    display: inline-flex;
    align-items: center;
    gap: var(--sp-1);
    flex: none;
    font-size: var(--fs);
    font-variant-numeric: tabular-nums;
    color: inherit;
  }
  .wc .st {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: var(--status-icon-slot);
    height: var(--status-icon-slot);
  }

  .ph {
    width: 40%;
    height: 6px;
    border-radius: 3px;
    background: var(--bg-3);
  }

  /* The 14px chevron sits under lane 0 (centered on cx(0) = 17: geometry PAD + 7), right of the branch / tag column. */
  .row.trunc {
    display: flex;
    padding-left: calc(var(--rw) + 10px);
    color: var(--text-2);
    background: none;
  }

  .handle {
    position: absolute;
    top: 0;
    bottom: 0;
    z-index: 3;
    width: 6px;
    cursor: col-resize;
  }
  .handle::after {
    content: '';
    position: absolute;
    top: 0;
    bottom: 0;
    left: 3px;
    width: 1px;
    background: transparent;
    transition: background var(--dur);
  }
  .handle:hover::after {
    background: var(--border-strong);
  }
  .handle.on::after {
    background: var(--accent);
  }

  .chip {
    position: absolute;
    left: 6px;
    bottom: 6px;
    z-index: 4;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 22px;
    height: 20px;
    border: 1px solid var(--border-strong);
    border-radius: var(--radius-sm);
    background: var(--bg-float);
    color: var(--accent);
  }
  .chip:hover {
    background: var(--bg-hover);
  }
</style>
