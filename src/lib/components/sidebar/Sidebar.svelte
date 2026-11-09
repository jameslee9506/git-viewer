<!--
  Refs sidebar: HEAD summary, ref filter (⌘⌥F) and one virtualized tree of local branches, remotes,
  worktrees, stashes, submodules and tags, the header of the section scrolled into pinned on top.
  The row model is built by the pure functions in tree.ts.
-->
<script module lang="ts">
  import { loadJSON, saveJSON } from '../../state/persist';
  import type { RepoStore } from '../../state/repo.svelte';
  import { defaultCollapsed } from './tree';

  const COLLAPSED_KEY = 'sidebar.collapsed';

  function readCollapsed(): Record<string, boolean> {
    const raw = loadJSON<unknown>(COLLAPSED_KEY, null);
    const out: Record<string, boolean> = {};
    if (raw && typeof raw === 'object' && !Array.isArray(raw)) {
      for (const [k, v] of Object.entries(raw)) if (typeof v === 'boolean') out[k] = v;
    }
    return out;
  }

  /** Collapse overrides (key → collapsed), persisted and shared by the sidebars of all tabs. */
  let collapsedOverrides = $state.raw<Record<string, boolean>>(readCollapsed());

  function isCollapsed(key: string): boolean {
    return collapsedOverrides[key] ?? defaultCollapsed(key);
  }

  function persistCollapsed(keys: string[], collapsed: boolean): void {
    const next = { ...collapsedOverrides };
    for (const k of keys) {
      if (collapsed === defaultCollapsed(k)) delete next[k];
      else next[k] = collapsed;
    }
    collapsedOverrides = next;
    saveJSON(COLLAPSED_KEY, next);
  }

  /**
   * The latest filter focus request (⌘⌥F): for the sidebar of the repo `id` (unique among open tabs);
   * `seq` tells requests apart. By id, not the store: a closed tab's store must not stay reachable.
   */
  let filterRequest = $state.raw<{ seq: number; id: string } | null>(null);
  /** Requests up to this seq are done: a sidebar mounted later must not act on one again. */
  let filterHandled = 0;

  /** Focus and select the ref filter of `repo`'s sidebar: at once, or as soon as that sidebar mounts. */
  export function requestFilterFocus(repo: RepoStore): void {
    filterRequest = { seq: (filterRequest?.seq ?? 0) + 1, id: repo.id };
  }

  /** The pending filter request is for `repo`'s sidebar: consume it. */
  function takeFilterRequest(repo: RepoStore): boolean {
    const req = filterRequest;
    if (!req || req.id !== repo.id || req.seq <= filterHandled) return false;
    filterHandled = req.seq;
    return true;
  }
</script>

<script lang="ts">
  import { revealItemInDir } from '@tauri-apps/plugin-opener';
  import { tick } from 'svelte';
  import AppWindow from '@lucide/svelte/icons/app-window';
  import Cherry from '@lucide/svelte/icons/cherry';
  import ChevronsDownUp from '@lucide/svelte/icons/chevrons-down-up';
  import ChevronsUpDown from '@lucide/svelte/icons/chevrons-up-down';
  import CloudOff from '@lucide/svelte/icons/cloud-off';
  import Copy from '@lucide/svelte/icons/copy';
  import Eye from '@lucide/svelte/icons/eye';
  import EyeOff from '@lucide/svelte/icons/eye-off';
  import FolderOpen from '@lucide/svelte/icons/folder-open';
  import FunnelX from '@lucide/svelte/icons/funnel-x';
  import GitBranch from '@lucide/svelte/icons/git-branch';
  import GitCommitHorizontal from '@lucide/svelte/icons/git-commit-horizontal';
  import GitMerge from '@lucide/svelte/icons/git-merge';
  import GitPullRequestArrow from '@lucide/svelte/icons/git-pull-request-arrow';
  import Mail from '@lucide/svelte/icons/mail';
  import Search from '@lucide/svelte/icons/search';
  import SearchX from '@lucide/svelte/icons/search-x';
  import Split from '@lucide/svelte/icons/split';
  import Target from '@lucide/svelte/icons/target';
  import Undo2 from '@lucide/svelte/icons/undo-2';
  import X from '@lucide/svelte/icons/x';
  import { api, errorMessage } from '../../api';
  import { app } from '../../state/app.svelte';
  import { getRepo, type Selection } from '../../state/repo.svelte';
  import { showContextMenu, toast, type MenuEntry } from '../../state/ui.svelte';
  import type { RepoOperation, Worktree } from '../../types';
  import { copyText, shortOid } from '../../util/format';
  import { isComposing } from '../../util/keys';
  import { rowTop, type RowMetrics } from '../common/rows';
  import VirtualList from '../common/VirtualList.svelte';
  import SidebarRow from './SidebarRow.svelte';
  import {
    buildSections,
    changeCount,
    expandable,
    flatten,
    hideableName,
    indexRows,
    innerFolderKeys,
    isCommitTag,
    outermostAncestor,
    pinnedHead,
    pinnedOffset,
    queryTerms,
    rowRefs,
    sectionHeads,
    sectionRows,
    trackLabel,
    type GroupRow,
    type LeafRow,
    type Row,
  } from './tree';

  const OP_ICONS: Record<RepoOperation['kind'], typeof GitMerge> = {
    merge: GitMerge,
    rebase: GitPullRequestArrow,
    'cherry-pick': Cherry,
    revert: Undo2,
    bisect: Split,
    am: Mail,
  };
  const SEP: MenuEntry = { separator: true };
  const ROW_H = 24;
  /** Section header rows are this much taller: 2px above and below the row content. */
  const SECTION_EXTRA = 4;
  /** At most this many other worktrees get a status query. */
  const MAX_WORKTREE_STATUS = 16;
  const NO_WORKTREES: Worktree[] = [];

  const repo = getRepo();

  let query = $state('');
  /** Groups collapsed while a filter is active (every match starts expanded). */
  let filterCollapsed = $state.raw<ReadonlySet<string>>(new Set());
  /** Key of the cursor row: keyboard position and last clicked row. */
  let cursorKey = $state<string | null>(null);
  let list = $state<ReturnType<typeof VirtualList>>();
  let input = $state<HTMLInputElement>();
  /** Changed-path counts of the other worktrees, by path. */
  let otherChanges = $state.raw<ReadonlyMap<string, number>>(new Map());
  let changesSeq = 0;
  /** The other-worktree status request in flight (settled either way), see the effect below. */
  let statusRead: Promise<unknown> = Promise.resolve();
  let scrollBeforeFilter = 0;
  /** Virtual scrollTop of the list, from its scroll events. */
  let scrollTop = $state(0);
  /** Width of the list's vertical scrollbar, which the pinned header leaves uncovered. */
  let gutter = $state(0);

  const terms = $derived(queryTerms(query));
  const filtering = $derived(terms.length > 0);
  const sections = $derived(repo.refs ? buildSections(repo.refs, query) : []);
  const rows = $derived(flatten(sections, filtering ? (k) => filterCollapsed.has(k) : isCollapsed));
  const rowIndex = $derived(indexRows(rows));
  /** Row → its section header's row; recomputed with the rows, so a scroll event costs O(1). */
  const heads = $derived(sectionHeads(rows));
  /** The section header rows: the list's tall rows. */
  const headRows = $derived(sectionRows(rows));
  const metrics = $derived<RowMetrics>({ rowHeight: ROW_H, tall: headRows, extra: SECTION_EXTRA });
  /** The section header pinned over the top of the list (-1: none) and its offset (≤ 0: pushed up). */
  const pinned = $derived(pinnedHead(heads, scrollTop, metrics));
  const pinnedY = $derived(pinned < 0 ? 0 : pinnedOffset(headRows, pinned, scrollTop, metrics));
  /** Refs matching the filter (every leaf, also those in collapsed groups). */
  const matchCount = $derived(sections.reduce((n, s) => n + s.count, 0));
  const cursor = $derived(cursorKey == null ? -1 : (rowIndex.get(cursorKey) ?? -1));
  const hiddenSet = $derived(new Set(repo.hiddenRefs));
  const onlySet = $derived(new Set(repo.onlyRefs));
  const hasFilters = $derived(repo.hiddenRefs.length > 0 || repo.onlyRefs.length > 0);

  const head = $derived(repo.refs?.head ?? repo.status?.head ?? null);
  const headBranch = $derived(repo.refs?.locals.find((b) => b.isHead) ?? null);
  const headTrack = $derived(
    repo.status
      ? trackLabel(repo.status.ahead, repo.status.behind)
      : headBranch
        ? trackLabel(headBranch.ahead, headBranch.behind)
        : '',
  );
  const headTitle = $derived.by(() => {
    if (!head) return '';
    if (head.detached) return head.oid ?? '';
    const upstream = repo.status?.upstream ?? headBranch?.upstream ?? null;
    const name = head.branch ?? '';
    return upstream ? `${name} → ${upstream}${headBranch?.upstreamGone ? ' (gone)' : ''}` : name;
  });
  const operation = $derived(repo.status?.operation ?? null);
  const currentChanges = $derived(repo.status ? changeCount(repo.status) : null);
  /**
   * The snapshot's worktrees: a new list only with a new snapshot. Branch counts merged in later
   * (repo.refs replaced by a copy, see RepoStore.#withTrack) keep it, so they re-read no status below.
   */
  const worktrees = $derived(repo.refs?.worktrees ?? NO_WORKTREES);
  /** Linked worktrees exist and their section is expanded (independent of the filter). */
  const worktreesOpen = $derived(worktrees.length > 1 && !isCollapsed('s:worktrees'));

  // Change counts of the other worktrees, re-read whenever refs reload while the section is open.
  // One `git status` at a time: the backend runs at most 8 git processes, and a burst of full
  // worktree scans must not hold up the graph reload that follows every refs reload. Also across
  // re-reads: refs reload (every commit, rebase step, fetch) faster than a big worktree's status
  // runs, and the superseded read's request cannot be cancelled, so a new read starts its first
  // request only once that one is done (all but the newest read then stop).
  $effect(() => {
    const targets = worktreesOpen
      ? worktrees.filter((w) => !w.bare && !w.isCurrent && !w.prunable).slice(0, MAX_WORKTREE_STATUS)
      : [];
    const seq = ++changesSeq;
    if (targets.length === 0) {
      otherChanges = new Map();
      return;
    }
    void (async () => {
      const next = new Map<string, number>();
      for (const w of targets) {
        await statusRead;
        if (seq !== changesSeq) return;
        const read = api.status(w.path);
        statusRead = read.catch(() => {});
        let n: number | null = null;
        try {
          n = changeCount(await read);
        } catch {
          // Unreadable (e.g. on an unmounted volume): no count.
        }
        if (seq !== changesSeq) return;
        if (n != null) next.set(w.path, n);
        // Shown as they arrive; worktrees not re-read yet keep their previous count.
        otherChanges = new Map([...otherChanges, ...next]);
      }
      otherChanges = next;
    })();
    return () => {
      changesSeq++;
    };
  });

  // ⌘⌥F (requestFilterFocus), also one made before this sidebar mounted.
  $effect(() => {
    const el = input;
    if (!el || !takeFilterRequest(repo)) return;
    el.focus();
    el.select();
  });

  // The scrollbar comes and goes with the content height (the list's content box changes with it).
  $effect(() => {
    const vp = list?.getViewport();
    if (!vp) return;
    const measure = () => {
      gutter = vp.offsetWidth - vp.clientWidth;
    };
    const ro = new ResizeObserver(measure);
    ro.observe(vp);
    measure();
    return () => ro.disconnect();
  });

  // ───────────── actions ─────────────

  function copy(text: string): void {
    void copyText(text).then(() => toast('Copied'));
  }

  function revealInFinder(path: string): void {
    revealItemInDir(path).catch((e: unknown) => toast(errorMessage(e)));
  }

  function openTab(path: string): void {
    void app.open(path, { newTab: true });
  }

  // Row lookups are async (graph_find, chunk fetches): a slow one must not override a later click
  // here or in the graph, so every sidebar selection checks it is still the latest before applying.
  let navSeq = 0;

  /** Start a sidebar selection; the check fails once a later one starts or the selection changes. */
  function begin(): () => boolean {
    const seq = ++navSeq;
    const sel = repo.selection;
    return () => seq === navSeq && repo.selection === sel;
  }

  /** Select graph row i (centered) unless `current()` fails while its chunk loads. */
  async function pickRow(i: number, current: () => boolean): Promise<void> {
    await repo.graph.getRow(i);
    if (current()) await repo.selectRow(i, 'center');
  }

  /**
   * Select the commit `oid` and center its graph row (as repo.reveal does); when it is not in the
   * graph (hidden, filtered out), select `fallback` without a row: the details still show it.
   */
  async function jumpTo(oid: string, fallback: Selection = { kind: 'commit', oid }): Promise<void> {
    const current = begin();
    const g = repo.graph;
    const i = await g.find(oid);
    if (!current()) return;
    if (i != null) {
      await pickRow(i, current);
      return;
    }
    g.selectedRow = null;
    g.compareRow = null;
    repo.select(fallback);
  }

  function revealHead(): void {
    if (head?.oid) void jumpTo(head.oid);
  }

  /** Show a worktree's working tree in the details; the graph scrolls to its HEAD row. */
  async function selectWorktree(w: Worktree): Promise<void> {
    if (w.prunable) {
      // Its directory is gone: only the commit it was on is left to show.
      if (w.head) await jumpTo(w.head);
      return;
    }
    const g = repo.graph;
    const wip = g.summary?.wipRow;
    if (w.isCurrent && wip != null) {
      await pickRow(wip, begin());
      return;
    }
    // The opened worktree is addressed by the repo id, so the graph's WIP row links up once dirty.
    const cwd = w.isCurrent ? repo.id : w.path;
    g.selectedRow = null;
    g.compareRow = null;
    repo.select({ kind: 'wip', cwd });
    // Taken after the selection change: only a later one cancels the scroll.
    const current = begin();
    if (!w.head) return;
    const i = await g.find(w.head);
    if (i != null && current()) g.scrollTo(i, 'center');
  }

  /** Click / Enter: groups toggle; refs select their tip in the graph. */
  function activate(r: Row): void {
    cursorKey = r.key;
    if (r.type !== 'leaf') {
      if (expandable(r)) setCollapsed([r.key], r.expanded);
      return;
    }
    const n = r.node;
    switch (n.kind) {
      case 'local':
      case 'remote':
        void jumpTo(n.ref.oid);
        break;
      case 'tag':
        // A tag on a tree or blob has no commit to show (its row is dimmed).
        if (isCommitTag(n.ref)) void jumpTo(n.ref.oid);
        break;
      case 'stash':
        // Not a graph row when stashes are hidden from the graph: then the stash alone.
        void jumpTo(n.ref.oid, { kind: 'stash', oid: n.ref.oid });
        break;
      case 'worktree':
        void selectWorktree(n.ref);
        break;
      case 'submodule':
        break;
    }
  }

  /** Double-click: open a worktree or an initialized submodule in a new tab. */
  function open(r: Row): void {
    if (r.type !== 'leaf') return;
    const n = r.node;
    if (n.kind === 'worktree' && !n.ref.isCurrent && !n.ref.prunable) openTab(n.ref.path);
    else if (n.kind === 'submodule' && n.ref.state !== 'uninitialized') openTab(`${repo.id}/${n.ref.path}`);
  }

  /** Eye / Space: a leaf toggles itself; a group hides everything below unless all of it is hidden. */
  function toggleHide(r: Row): void {
    const names = rowRefs(r);
    if (names.length === 0) return;
    if (r.type === 'leaf') repo.toggleHidden(names[0]);
    else repo.setHidden(names, !names.every((n) => hiddenSet.has(n)));
  }

  function solo(r: Row): void {
    const name = r.type === 'leaf' ? hideableName(r.node) : null;
    if (name) repo.solo(name);
  }

  function setCollapsed(keys: string[], collapsed: boolean): void {
    if (collapsed && cursorKey != null) {
      // Keep the cursor on a visible row: lift it to the outermost group collapsing around it.
      const lifted = outermostAncestor(rows, rowIndex, cursorKey, new Set(keys));
      if (lifted) cursorKey = lifted;
    }
    if (!filtering) {
      persistCollapsed(keys, collapsed);
      return;
    }
    const next = new Set(filterCollapsed);
    for (const k of keys) {
      if (collapsed) next.add(k);
      else next.delete(k);
    }
    filterCollapsed = next;
  }

  // ───────────── context menus ─────────────

  function copyEntry(label: string, text: string): MenuEntry {
    return { label, icon: Copy, action: () => copy(text) };
  }

  function refEntries(fullName: string): MenuEntry[] {
    const hidden = hiddenSet.has(fullName);
    const soloed = onlySet.has(fullName);
    return [
      {
        label: hidden ? 'Show' : 'Hide',
        icon: hidden ? Eye : EyeOff,
        hint: 'Space',
        action: () => repo.toggleHidden(fullName),
      },
      { label: soloed ? 'Unsolo' : 'Solo', icon: Target, hint: '⌥Space', action: () => repo.solo(fullName) },
      { label: 'Clear filters', icon: FunnelX, disabled: !hasFilters, action: () => repo.clearFilters() },
    ];
  }

  function groupEntries(r: GroupRow): MenuEntry[] {
    const items: MenuEntry[] = [];
    const url = r.type === 'folder' ? r.node.remote?.url : null;
    if (url) items.push(copyEntry('Copy URL', url));
    const names = rowRefs(r);
    if (names.length > 0) {
      if (items.length > 0) items.push(SEP);
      if (names.some((n) => !hiddenSet.has(n))) {
        items.push({ label: 'Hide all', icon: EyeOff, action: () => repo.setHidden(names, true) });
      }
      if (names.some((n) => hiddenSet.has(n))) {
        items.push({ label: 'Show all', icon: Eye, action: () => repo.setHidden(names, false) });
      }
    }
    const inner = innerFolderKeys(r);
    if (inner.length > 0) {
      if (items.length > 0) items.push(SEP);
      items.push(
        { label: 'Expand all', icon: ChevronsUpDown, action: () => setCollapsed([r.key, ...inner], false) },
        { label: 'Collapse all', icon: ChevronsDownUp, action: () => setCollapsed(inner, true) },
      );
    }
    return items;
  }

  function leafEntries(r: LeafRow): MenuEntry[] {
    const n = r.node;
    switch (n.kind) {
      case 'local': {
        const b = n.ref;
        const items = [copyEntry('Copy name', b.name), copyEntry('Copy SHA', b.oid), SEP, ...refEntries(b.fullName)];
        const wt = b.worktree;
        if (wt) {
          // A prunable worktree's directory is gone: nothing to open or reveal.
          const gone = worktrees.some((w) => w.path === wt && w.prunable);
          items.push(SEP);
          if (!b.isHead) {
            items.push({ label: 'Open worktree', icon: AppWindow, disabled: gone, action: () => openTab(wt) });
          }
          items.push({ label: 'Reveal in Finder', icon: FolderOpen, disabled: gone, action: () => revealInFinder(wt) });
        }
        return items;
      }
      case 'remote':
        return [copyEntry('Copy name', n.ref.name), copyEntry('Copy SHA', n.ref.oid), SEP, ...refEntries(n.ref.fullName)];
      case 'tag': {
        const t = n.ref;
        const items = [copyEntry('Copy name', t.name)];
        if (t.annotated && t.message) items.push(copyEntry('Copy message', t.message));
        items.push(copyEntry('Copy SHA', t.oid));
        // Never in the graph when it is on a tree or blob: nothing to hide or solo.
        if (isCommitTag(t)) items.push(SEP, ...refEntries(t.fullName));
        return items;
      }
      case 'stash':
        return [copyEntry('Copy name', n.ref.name), copyEntry('Copy message', n.ref.message)];
      case 'worktree': {
        const w = n.ref;
        return [
          {
            label: 'Open in new tab',
            icon: AppWindow,
            disabled: w.isCurrent || w.prunable,
            action: () => openTab(w.path),
          },
          { label: 'Reveal in Finder', icon: FolderOpen, disabled: w.prunable, action: () => revealInFinder(w.path) },
          SEP,
          copyEntry('Copy path', w.path),
        ];
      }
      case 'submodule': {
        const m = n.ref;
        const path = `${repo.id}/${m.path}`;
        return [
          {
            label: 'Open in new tab',
            icon: AppWindow,
            disabled: m.state === 'uninitialized',
            action: () => openTab(path),
          },
          { label: 'Reveal in Finder', icon: FolderOpen, action: () => revealInFinder(path) },
          SEP,
          copyEntry('Copy path', m.path),
        ];
      }
    }
  }

  // ───────────── rows: mouse ─────────────

  /** Per-row state that depends on the repo's ref filters and worktree status. */
  function flags(r: Row) {
    const names = rowRefs(r);
    const leaf = r.type === 'leaf' ? r.node : null;
    const own = leaf ? hideableName(leaf) : null;
    const soloed = own != null && onlySet.has(own);
    let changes: number | null = null;
    if (leaf?.kind === 'worktree') {
      changes = leaf.ref.isCurrent ? currentChanges : (otherChanges.get(leaf.ref.path) ?? null);
    }
    // A tag on a tree or blob is never in the graph.
    const offGraph = leaf?.kind === 'tag' && !isCommitTag(leaf.ref);
    return {
      hideable: names.length > 0,
      hidden: names.length > 0 && names.every((n) => hiddenSet.has(n)),
      soloed,
      // HEAD is always walked, so a solo never drops the checked-out branch.
      dimmed:
        offGraph || (own != null && !soloed && onlySet.size > 0 && !(leaf?.kind === 'local' && leaf.ref.isHead)),
      changes,
    };
  }

  function onRowClick(r: Row, e: MouseEvent): void {
    // ⌃-click is the macOS context click (the menu handles it); the first click of a double-click
    // already acted.
    if (e.ctrlKey || e.detail > 1) return;
    if (e.altKey && r.type === 'leaf' && hideableName(r.node)) {
      cursorKey = r.key;
      solo(r);
      return;
    }
    activate(r);
  }

  function onRowMenu(r: Row, e: MouseEvent): void {
    cursorKey = r.key;
    const items = r.type === 'leaf' ? leafEntries(r) : groupEntries(r);
    if (items.length > 0) showContextMenu(e, items);
    else e.preventDefault();
  }

  /** The pinned header lies outside the list: a press on it focuses the list, as one on a row does. */
  function onPinnedDown(e: MouseEvent): void {
    e.preventDefault();
    list?.focus();
  }

  /** Click on the pinned header (row i): toggle its section like the header row does. */
  function onPinnedClick(r: Row, i: number, e: MouseEvent): void {
    if (e.ctrlKey || e.detail > 1) return;
    activate(r);
    // Folded up, the section no longer reaches the top of the list: bring its header row there.
    void tick().then(() => list?.scrollToIndex(i, 'start'));
  }

  /** Nothing scrolls under the pinned header (it is outside the list's scroller): scroll the list. */
  function onPinnedWheel(e: WheelEvent): void {
    if (!list || e.deltaY === 0) return;
    const unit =
      e.deltaMode === WheelEvent.DOM_DELTA_LINE
        ? ROW_H
        : e.deltaMode === WheelEvent.DOM_DELTA_PAGE
          ? list.pageSize() * ROW_H
          : 1;
    list.setScrollTop(list.getScrollTop() + e.deltaY * unit);
  }

  // ───────────── keyboard ─────────────

  /** Scroll row j into view, clear of the pinned section header. */
  function reveal(j: number): void {
    if (!list) return;
    list.scrollToIndex(j);
    // Within the pinned header's height of the top edge, a row sits under it: scroll up by that much
    // (the header itself has no pinned stand-in while it shows).
    if (rows[j]?.type === 'section') return;
    const pinnedH = ROW_H + SECTION_EXTRA;
    const y = rowTop(metrics, j) - list.getScrollTop();
    if (y < pinnedH) list.setScrollTop(Math.max(0, list.getScrollTop() - (pinnedH - y)));
  }

  function moveTo(i: number): void {
    if (rows.length === 0) return;
    const j = Math.max(0, Math.min(rows.length - 1, i));
    cursorKey = rows[j].key;
    reveal(j);
  }

  function onListKey(e: KeyboardEvent): void {
    if (isComposing(e)) return;
    if (e.code === 'Escape') {
      // The ref filter is the first Esc layer here; with none, the app's layers take over.
      if (!query || e.metaKey || e.ctrlKey || e.altKey || e.shiftKey) return;
      setQuery('');
      e.preventDefault();
      return;
    }
    if (rows.length === 0) return;
    const cur = cursor;
    const r = cur >= 0 ? rows[cur] : null;
    const mod = e.metaKey || e.ctrlKey || e.altKey;
    switch (e.code) {
      case 'ArrowDown':
      case 'ArrowUp': {
        if (e.ctrlKey || e.altKey) return;
        const down = e.code === 'ArrowDown';
        if (e.metaKey) moveTo(down ? rows.length - 1 : 0);
        else if (cur < 0) moveTo(0);
        else if (!down && cur === 0) input?.focus();
        else moveTo(cur + (down ? 1 : -1));
        break;
      }
      case 'KeyJ':
      case 'KeyK':
        if (mod) return;
        moveTo(cur < 0 ? 0 : cur + (e.code === 'KeyJ' ? 1 : -1));
        break;
      case 'PageDown':
      case 'PageUp': {
        if (mod) return;
        const page = list?.pageSize() ?? 1;
        moveTo(cur < 0 ? 0 : cur + (e.code === 'PageDown' ? page : -page));
        break;
      }
      case 'Home':
      case 'End':
        if (mod) return;
        moveTo(e.code === 'Home' ? 0 : rows.length - 1);
        break;
      case 'ArrowLeft':
        if (mod || !r) return;
        if (r.type !== 'leaf' && r.expanded && expandable(r)) setCollapsed([r.key], true);
        else if (r.parent != null) moveTo(rowIndex.get(r.parent) ?? cur);
        break;
      case 'ArrowRight':
        if (mod || !r || r.type === 'leaf' || !expandable(r)) return;
        if (r.expanded) moveTo(cur + 1);
        else setCollapsed([r.key], false);
        break;
      case 'Enter':
      case 'NumpadEnter':
        if (mod || !r) return;
        activate(r);
        break;
      case 'Space':
        if (e.metaKey || e.ctrlKey || !r) return;
        // A held Space toggles once: every toggle starts a full graph build.
        if (e.repeat) break;
        if (e.altKey) solo(r);
        else toggleHide(r);
        break;
      default:
        return;
    }
    e.preventDefault();
  }

  // ───────────── filter ─────────────

  function setQuery(value: string): void {
    const was = filtering;
    // A cursor among the filter results stays in view when the filter is cleared.
    const keep = was && cursor >= 0 ? cursorKey : null;
    if (!was) scrollBeforeFilter = list?.getScrollTop() ?? 0;
    query = value;
    filterCollapsed = new Set();
    if (filtering) {
      list?.scrollToIndex(0, 'start');
    } else if (was) {
      void tick().then(() => {
        const i = keep == null ? undefined : rowIndex.get(keep);
        if (i != null) {
          list?.scrollToIndex(i, 'center');
          return;
        }
        list?.setScrollTop(scrollBeforeFilter);
      });
    }
  }

  function onFilterKey(e: KeyboardEvent): void {
    if (isComposing(e)) return;
    switch (e.code) {
      case 'Escape':
        if (query) setQuery('');
        else {
          input?.blur();
          list?.focus();
        }
        break;
      case 'ArrowDown':
        if (rows.length === 0) return;
        if (cursor < 0) moveTo(Math.max(0, rows.findIndex((r) => r.type === 'leaf')));
        else reveal(cursor);
        list?.focus();
        break;
      case 'Enter':
      case 'NumpadEnter': {
        // The highlighted row, else (only while filtering) the first match.
        const i =
          cursor >= 0 && rows[cursor].type === 'leaf'
            ? cursor
            : filtering
              ? rows.findIndex((r) => r.type === 'leaf')
              : -1;
        if (i < 0) return;
        moveTo(i);
        activate(rows[i]);
        break;
      }
      default:
        return;
    }
    e.preventDefault();
    e.stopPropagation();
  }

  function clearFilter(): void {
    setQuery('');
    input?.focus();
  }
</script>

<div class="sidebar">
  <header class="head">
    {#if head}
      <button class="head-ref" title={headTitle} disabled={!head.oid} onclick={revealHead}>
        {#if head.detached}
          <GitCommitHorizontal size={14} strokeWidth={1.75} />
          <span class="mono">{head.oid ? shortOid(head.oid) : ''}</span>
        {:else}
          <GitBranch size={14} strokeWidth={1.75} />
          <span class="branch ellipsis" class:unborn={head.unborn}>{head.branch ?? ''}</span>
        {/if}
        {#if headBranch?.upstreamGone}
          <span class="gone"><CloudOff size={12} strokeWidth={1.75} /></span>
        {:else if headTrack}
          <span class="track">{headTrack}</span>
        {/if}
        {#if operation}
          {@const OpIcon = OP_ICONS[operation.kind]}
          <span class="op" title={operation.detail ? `${operation.kind} ${operation.detail}` : operation.kind}>
            <OpIcon size={12} strokeWidth={1.75} />
            {#if operation.step != null && operation.total != null}{operation.step}/{operation.total}{/if}
          </span>
        {/if}
      </button>
    {/if}
  </header>

  <div class="filter">
    <!-- The whole box focuses the input. -->
    <label class="fbox">
      <span class="ficon"><Search size={14} strokeWidth={1.75} /></span>
      <input
        bind:this={input}
        value={query}
        placeholder="Filter"
        spellcheck="false"
        autocomplete="off"
        autocorrect="off"
        autocapitalize="off"
        aria-label="Filter refs"
        oninput={(e) => setQuery(e.currentTarget.value)}
        onkeydown={onFilterKey}
      />
      {#if query}
        {#if filtering}<span class="hits">{matchCount}</span>{/if}
        <button class="clear" tabindex="-1" title="Clear" onclick={clearFilter}>
          <X size={12} strokeWidth={1.75} />
        </button>
      {:else}
        <span class="hint">⌘⌥F</span>
      {/if}
    </label>
  </div>

  <div class="tree sb-tree" role="tree" aria-label="Refs">
    <VirtualList
      bind:this={list}
      class="sb-list"
      count={rows.length}
      rowHeight={ROW_H}
      tall={headRows}
      tallExtra={SECTION_EXTRA}
      overscan={10}
      tabindex={0}
      onkeydown={onListKey}
      onscroll={(top) => (scrollTop = top)}
    >
      {#snippet row(i: number)}
        {@const r = rows[i]}
        {#if r}
          {@render item(r, i, i > 0 && r.type === 'section', (e) => onRowClick(r, e))}
        {/if}
      {/snippet}
    </VirtualList>
    {#if pinned >= 0}
      {@const r = rows[pinned]}
      <!-- A stand-in for the header row (that one stays in the list and the a11y tree). -->
      <div
        class="pinned"
        role="presentation"
        aria-hidden="true"
        style:right="{gutter}px"
        style:height="{ROW_H + SECTION_EXTRA}px"
        style:transform={pinnedY < 0 ? `translateY(${pinnedY}px)` : null}
        onmousedown={onPinnedDown}
        onwheel={onPinnedWheel}
      >
        {@render item(r, pinned, false, (e) => onPinnedClick(r, pinned, e))}
      </div>
    {/if}
    {#if filtering && rows.length === 0}
      <div class="empty none"><SearchX size={16} strokeWidth={1.75} /></div>
    {/if}
  </div>
</div>

{#snippet item(r: Row, i: number, separated: boolean, onclick: (e: MouseEvent) => void)}
  {@const f = flags(r)}
  <SidebarRow
    row={r}
    cursor={i === cursor}
    {separated}
    hidden={f.hidden}
    soloed={f.soloed}
    dimmed={f.dimmed}
    hideable={f.hideable}
    changes={f.changes}
    {terms}
    {onclick}
    ondblclick={() => open(r)}
    oncontextmenu={(e) => onRowMenu(r, e)}
    onhide={() => toggleHide(r)}
    onsolo={() => solo(r)}
  />
{/snippet}

<style>
  .sidebar {
    display: flex;
    flex-direction: column;
    height: 100%;
    min-width: 0;
    background: var(--bg-2);
  }

  /* Fixed height: the filter and list stay put while refs load. */
  .head {
    flex: none;
    display: flex;
    align-items: center;
    height: 34px;
    min-width: 0;
    padding: 6px 8px 2px;
  }
  .head-ref {
    flex: 1 1 auto;
    display: flex;
    align-items: center;
    gap: 5px;
    height: 26px;
    min-width: 0;
    padding: 0 8px;
    border-radius: var(--radius-sm);
    font-size: var(--fs);
    color: var(--text-1);
    text-align: left;
  }
  .head-ref:hover:not(:disabled) {
    background: var(--bg-hover);
    color: var(--text-0);
  }
  .head-ref :global(svg) {
    flex: none;
  }
  .branch {
    flex: 0 1 auto;
  }
  .branch.unborn {
    color: var(--text-2);
  }
  .track {
    flex: none;
    font-family: var(--font-mono);
    font-size: var(--fs-xs);
    color: var(--text-2);
  }
  .gone {
    flex: none;
    display: inline-flex;
    color: var(--text-2);
  }
  .op {
    flex: none;
    display: inline-flex;
    align-items: center;
    gap: 3px;
    margin-left: auto;
    font-family: var(--font-mono);
    font-size: var(--fs-xs);
    color: var(--warn);
  }

  .filter {
    flex: none;
    padding: 2px 8px 8px;
  }
  /* Search field: icon, input, then the ⌘⌥F hint, or the match count and clear button. */
  .fbox {
    display: flex;
    align-items: center;
    gap: 6px;
    height: 28px;
    padding: 0 4px 0 8px;
    border: 1px solid var(--input-border);
    border-radius: var(--radius-sm);
    background: var(--bg-input);
    cursor: text;
  }
  .fbox:focus-within {
    border-color: var(--focus-ring);
  }
  .fbox input {
    flex: 1 1 auto;
    min-width: 0;
    height: 100%;
    padding: 0;
    border: 0;
    outline: none;
    background: transparent;
    font-size: var(--fs);
    color: var(--text-0);
  }
  .fbox input::placeholder {
    color: var(--text-2);
  }
  .ficon {
    flex: none;
    display: inline-flex;
    color: var(--text-2);
  }
  .fbox:focus-within .ficon {
    color: var(--text-1);
  }
  .hint {
    flex: none;
    padding-right: 4px;
    font-size: var(--fs-xs);
    color: var(--text-2);
  }
  .fbox:focus-within .hint {
    display: none;
  }
  .hits {
    flex: none;
    font-family: var(--font-mono);
    font-size: var(--fs-xs);
    color: var(--text-2);
  }
  .clear {
    flex: none;
    width: 20px;
    height: 20px;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    border-radius: var(--radius-sm);
    color: var(--text-2);
    cursor: default;
  }
  .clear:hover {
    color: var(--text-0);
    background: var(--bg-hover);
  }

  .tree {
    position: relative;
    flex: 1 1 auto;
    min-height: 0;
    overflow: hidden;
  }
  /* The header of the section at the top edge, pinned over the list (its scrollbar left free). */
  .pinned {
    position: absolute;
    top: 0;
    left: 0;
    z-index: 1;
    box-shadow: 0 1px 0 var(--border);
  }
  .none {
    position: absolute;
    inset: 0;
    pointer-events: none;
  }
</style>
