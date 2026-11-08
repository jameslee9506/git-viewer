<!--
  Files at a revision (`repo.view` kind 'tree'): a lazily loaded folder tree on the left (one
  listing per expanded folder; typing a filter switches to a single recursive listing filtered
  client-side) and the selected file on the right. A new rev keeps the expanded folders and the
  shown file when they still exist; the old listing stays until the new one is complete.
-->
<script module lang="ts">
  import type { TreeEntry } from '../../types';

  /**
   * The state a tree view was left in (expanded folders, selection, shown file), by its view object: a
   * viewer opened over it remembers that object and `repo.back()` brings the same object back to a
   * new instance.
   */
  const leftAt = new WeakMap<object, { expanded: Set<string>; selPath: string | null; shown: TreeEntry | null }>();
</script>

<script lang="ts">
  import ArrowLeft from '@lucide/svelte/icons/arrow-left';
  import Boxes from '@lucide/svelte/icons/boxes';
  import ChevronRight from '@lucide/svelte/icons/chevron-right';
  import Copy from '@lucide/svelte/icons/copy';
  import FileIcon from '@lucide/svelte/icons/file';
  import FileSymlink from '@lucide/svelte/icons/file-symlink';
  import Folder from '@lucide/svelte/icons/folder';
  import FolderOpen from '@lucide/svelte/icons/folder-open';
  import FolderTree from '@lucide/svelte/icons/folder-tree';
  import Hash from '@lucide/svelte/icons/hash';
  import RotateCcwClock from '@lucide/svelte/icons/rotate-ccw-clock';
  import ScanText from '@lucide/svelte/icons/scan-text';
  import Search from '@lucide/svelte/icons/search';
  import TriangleExclamationPoint from '@lucide/svelte/icons/triangle-exclamation-point';
  import X from '@lucide/svelte/icons/x';
  import { onMount, tick, untrack } from 'svelte';
  import { api, errorMessage } from '../../api';
  import { getRepo } from '../../state/repo.svelte';
  import { showContextMenu, toast, type MenuEntry } from '../../state/ui.svelte';
  import type { BlobSpec } from '../../types';
  import { basename, copyText, dirname, shortOid } from '../../util/format';
  import Spinner from '../common/Spinner.svelte';
  import VirtualList from '../common/VirtualList.svelte';
  import { isComposing, isEditable } from '../../util/keys';
  import FileViewer, { escapeBack, shortRev } from './FileViewer.svelte';

  interface Row {
    e: TreeEntry;
    depth: number;
  }

  /** Recursive listing of one revision, for the filter. */
  interface AllFiles {
    rev: string;
    entries: TreeEntry[];
    lower: string[];
  }

  type Listed = { ok: true; list: TreeEntry[] } | { ok: false; error: string };

  const ROW_H = 22;
  const INDENT = 12;
  /** Keyboard moves update the viewer after this idle time (fast key repeat). */
  const FOLLOW_MS = 60;

  const repo = getRepo();
  /** The view this instance opens with, and where it was left when back() brings it back. */
  const initial = repo.view;
  const left = leftAt.get(initial);
  /** Revision of the listings shown. */
  let rev = $state<string | null>(null);
  /** Loaded listings by folder path ('' = root); replaced wholesale. */
  let dirs = $state.raw(new Map<string, TreeEntry[]>());
  let failed = $state.raw(new Map<string, string>());
  let busy = $state.raw(new Set<string>());
  /** Expanded folders; invariant: every ancestor of an expanded folder is expanded. */
  let expanded = $state.raw(new Set<string>(left?.expanded));
  let selPath = $state<string | null>(left?.selPath ?? null);
  /** Entry shown on the right (a blob or a submodule); kept while a folder is selected. */
  let shown = $state.raw<TreeEntry | null>(left?.shown ?? null);
  let filter = $state('');
  let all = $state.raw<AllFiles | null>(null);
  /** Revision whose recursive listing is in flight. */
  let allRev = $state<string | null>(null);
  let allError = $state<string | null>(null);
  /** Revision whose recursive listing failed (not retried until the filter is left). */
  let allFailed: string | null = null;
  let rootError = $state<string | null>(null);
  /** A different revision is being listed (old tree stays until it is complete). */
  let switching = $state(false);
  let list: VirtualList | undefined = $state();
  let input: HTMLInputElement | undefined = $state();
  let viewer: FileViewer | undefined = $state();
  let navSeq = 0;
  /** Previous filter query (detects leaving filter mode). */
  let lastQuery = '';
  let showTimer: ReturnType<typeof setTimeout> | undefined;
  const inflight = new Map<string, Promise<TreeEntry[] | null>>();
  /** View object of the latest navigation (see leftAt). */
  let navView: object | null = null;

  const query = $derived(filter.trim().toLowerCase());

  const rows = $derived.by<Row[]>(() => {
    if (query) {
      const a = all;
      if (!a || a.rev !== rev) return [];
      const terms = query.split(/\s+/);
      const out: Row[] = [];
      for (let i = 0; i < a.entries.length; i++) {
        const p = a.lower[i];
        if (terms.every((t) => p.includes(t))) out.push({ e: a.entries[i], depth: 0 });
      }
      return out;
    }
    const out: Row[] = [];
    const walk = (dir: string, depth: number) => {
      const items = dirs.get(dir);
      if (!items) return;
      for (const e of items) {
        out.push({ e, depth });
        if (e.kind === 'tree' && expanded.has(e.path)) walk(e.path, depth + 1);
      }
    };
    walk('', 0);
    return out;
  });

  const selIndex = $derived(selPath == null ? -1 : rows.findIndex((r) => r.e.path === selPath));

  const crumbs = $derived.by(() => {
    const p = selPath ?? shown?.path ?? null;
    if (!p) return [];
    const parts = p.split('/');
    return parts.map((name, i) => ({ name, path: parts.slice(0, i + 1).join('/') }));
  });

  const shownOid = $derived(shown?.kind === 'blob' ? shown.oid : null);
  const spec = $derived<BlobSpec | null>(shownOid ? { kind: 'oid', oid: shownOid } : null);

  $effect(() => {
    const v = repo.view;
    if (v.kind !== 'tree') return;
    navView = v;
    // Back where it was left: reveal the selection it had (the listings keep its expanded folders).
    const path = v === initial && left ? (left.selPath ?? v.path) : v.path;
    untrack(() => void navigate(v.rev, path));
  });

  // Fetch listings of expanded folders that are not loaded yet (after expanding or a rev switch).
  $effect(() => {
    const r = rev;
    const ex = expanded;
    const ds = dirs;
    if (r == null) return;
    untrack(() => {
      for (const d of ex) if (!ds.has(d) && !failed.has(d)) void loadDir(r, d);
    });
  });

  // The filter searches one recursive listing of the revision.
  $effect(() => {
    const r = rev;
    if (!query || r == null) return;
    untrack(() => void loadAll(r));
  });

  // Leaving filter mode (cleared or erased) reveals the selection in the tree.
  $effect(() => {
    const q = query;
    const was = lastQuery;
    lastQuery = q;
    if (!was || q) return;
    untrack(() => {
      allFailed = null;
      allError = null;
      if (selPath) void revealPath(selPath, navSeq);
    });
  });

  onMount(() => {
    if (!isEditable(document.activeElement)) list?.focus();
    const off = escapeBack(repo);
    return () => {
      // A viewer opened over this one keeps its view object for back(): remember the state.
      if (navView) leftAt.set(navView, { expanded, selPath, shown });
      off();
      clearTimeout(showTimer);
    };
  });

  function lookup(m: Map<string, TreeEntry[]>, path: string): TreeEntry | null {
    return m.get(dirname(path))?.find((e) => e.path === path) ?? null;
  }

  function listing(r: string, dir: string): Promise<Listed> {
    return api.listTree(repo.id, r, dir).then(
      (list): Listed => ({ ok: true, list }),
      (e): Listed => ({ ok: false, error: errorMessage(e) }),
    );
  }

  /** Show `nextRev` (keeping expanded folders and the shown file that still exist), then reveal `path`. */
  async function navigate(nextRev: string, path: string | null): Promise<void> {
    const gen = ++navSeq;
    // A newer navigation supersedes a revision switch still in flight (it never finishes).
    switching = false;
    // Also re-list when the root listing failed (navigating to the same rev again retries).
    if (nextRev !== rev || rootError != null) {
      switching = true;
      clearTimeout(showTimer);
      const want = ['', ...expanded];
      const shownDir = shown ? dirname(shown.path) : null;
      if (shownDir != null && !want.includes(shownDir)) want.push(shownDir);
      const results = await Promise.all(want.map((d) => listing(nextRev, d)));
      if (gen !== navSeq) return;
      switching = false;
      const head = results[0];
      const m = new Map<string, TreeEntry[]>();
      const ex = new Set<string>();
      want.forEach((d, i) => {
        const res = results[i];
        if (!res.ok) return;
        m.set(d, res.list);
        if (d && expanded.has(d)) ex.add(d);
      });
      // Drop folders whose parent vanished (keeps the expansion invariant).
      for (const d of ex) if (dirname(d) && !ex.has(dirname(d))) ex.delete(d);
      rev = nextRev;
      dirs = m;
      expanded = head.ok ? ex : new Set();
      failed = new Map();
      busy = new Set();
      inflight.clear();
      rootError = head.ok ? null : head.error;
      all = all?.rev === nextRev ? all : null;
      allError = null;
      allFailed = null;
      if (selPath != null && !lookup(m, selPath)) selPath = null;
      if (shown) {
        const e = lookup(m, shown.path);
        shown = e && e.kind !== 'tree' ? e : null;
      }
    }
    if (path) await revealPath(path, gen);
    else if (gen === navSeq && selPath != null) {
      await tick();
      scrollToSel('center');
    }
  }

  function loadDir(r: string, dir: string): Promise<TreeEntry[] | null> {
    if (r === rev) {
      const have = dirs.get(dir);
      if (have) return Promise.resolve(have);
    }
    const k = `${r}\0${dir}`;
    const pending = inflight.get(k);
    if (pending) return pending;
    if (r === rev) busy = new Set(busy).add(dir);
    const p = listing(r, dir).then((res) => {
      inflight.delete(k);
      if (r !== rev) return res.ok ? res.list : null;
      if (busy.has(dir)) {
        const b = new Set(busy);
        b.delete(dir);
        busy = b;
      }
      if (!res.ok) {
        failed = new Map(failed).set(dir, res.error);
        return null;
      }
      dirs = new Map(dirs).set(dir, res.list);
      return res.list;
    });
    inflight.set(k, p);
    return p;
  }

  async function loadAll(r: string): Promise<void> {
    // A failed listing is retried when the filter is entered again, not on every keystroke.
    if (all?.rev === r || allRev === r || allFailed === r) return;
    allRev = r;
    allError = null;
    try {
      const entries = await api.listTree(repo.id, r, '', true);
      if (r === rev) all = { rev: r, entries, lower: entries.map((e) => e.path.toLowerCase()) };
    } catch (e) {
      if (r === rev) {
        allError = errorMessage(e);
        allFailed = r;
      }
    } finally {
      if (allRev === r) allRev = null;
    }
  }

  /** Expand the ancestors of `path`, select it and scroll it into view (leaves filter mode). */
  async function revealPath(path: string, gen: number): Promise<void> {
    const r = rev;
    if (r == null) return;
    const parts = path.split('/').filter(Boolean);
    const chain = parts.map((_, i) => parts.slice(0, i + 1).join('/'));
    await Promise.all(['', ...chain.slice(0, -1)].map((d) => loadDir(r, d)));
    if (gen !== navSeq || r !== rev) return;
    filter = '';
    const ex = new Set(expanded);
    let found: TreeEntry | null = null;
    for (const p of chain) {
      const e = lookup(dirs, p);
      if (!e) break;
      found = e;
      if (e.kind !== 'tree') break;
      ex.add(p);
    }
    expanded = ex;
    if (!found) return;
    select(found, true);
    await tick();
    scrollToSel('center');
  }

  function select(e: TreeEntry, immediate: boolean): void {
    selPath = e.path;
    clearTimeout(showTimer);
    if (e.kind === 'tree') return;
    if (immediate) shown = e;
    else showTimer = setTimeout(() => (shown = e), FOLLOW_MS);
  }

  function scrollToSel(align: 'auto' | 'center' = 'auto'): void {
    if (selIndex >= 0) list?.scrollToIndex(selIndex, align);
  }

  function toggle(path: string, open = !expanded.has(path)): void {
    const ex = new Set(expanded);
    if (open) {
      ex.add(path);
      if (failed.has(path)) {
        const f = new Map(failed);
        f.delete(path);
        failed = f;
      }
    } else {
      for (const d of expanded) if (d === path || d.startsWith(`${path}/`)) ex.delete(d);
      if (selPath?.startsWith(`${path}/`)) selPath = path;
    }
    expanded = ex;
  }

  function move(to: number): void {
    const n = rows.length;
    if (n === 0) return;
    const i = Math.max(0, Math.min(n - 1, to));
    select(rows[i].e, false);
    list?.scrollToIndex(i);
  }

  function clickRow(r: Row): void {
    select(r.e, true);
    if (r.e.kind === 'tree' && !query) toggle(r.e.path);
    list?.focus();
  }

  /** Focus the file viewer; the first file shown mounts it on the next render (the text loads later). */
  async function focusViewer(): Promise<void> {
    await tick();
    viewer?.focus();
  }

  /** Leave filter mode (the selection is then revealed in the tree). */
  function clearFilter(): void {
    filter = '';
    list?.focus();
  }

  function onKey(e: KeyboardEvent): void {
    if (isComposing(e)) return;
    const n = rows.length;
    const plain = !e.metaKey && !e.ctrlKey && !e.altKey;
    const i = selIndex;
    const r = i >= 0 ? rows[i] : null;
    switch (e.code) {
      case 'ArrowDown':
        if (e.altKey || e.ctrlKey) return;
        move(e.metaKey ? n - 1 : i + 1);
        break;
      case 'ArrowUp':
        if (e.altKey || e.ctrlKey) return;
        move(e.metaKey || i < 0 ? 0 : i - 1);
        break;
      case 'KeyJ':
        if (!plain) return;
        move(i + 1);
        break;
      case 'KeyK':
        if (!plain) return;
        move(i < 0 ? 0 : i - 1);
        break;
      case 'PageDown':
        move(i + (list?.pageSize() ?? 10));
        break;
      case 'PageUp':
        move(i - (list?.pageSize() ?? 10));
        break;
      case 'Home':
        move(0);
        break;
      case 'End':
        move(n - 1);
        break;
      case 'ArrowRight':
        if (!r || !plain) return;
        if (r.e.kind !== 'tree') {
          select(r.e, true);
          void focusViewer();
        } else if (query) return;
        else if (!expanded.has(r.e.path)) toggle(r.e.path, true);
        else if ((rows[i + 1]?.depth ?? -1) > r.depth) move(i + 1);
        break;
      case 'ArrowLeft': {
        if (!r || query) return;
        if (r.e.kind === 'tree' && expanded.has(r.e.path)) toggle(r.e.path, false);
        else {
          const parent = dirname(r.e.path);
          const j = parent ? rows.findIndex((x) => x.e.path === parent) : -1;
          if (j >= 0) move(j);
        }
        break;
      }
      case 'Enter':
      case 'Space':
        if (!r) return;
        if (r.e.kind === 'tree') toggle(r.e.path);
        else {
          select(r.e, true);
          if (e.code === 'Enter') {
            if (query) clearFilter();
            else void focusViewer();
          }
        }
        break;
      case 'Slash':
        if (!plain) return;
        input?.focus();
        input?.select();
        break;
      case 'Escape':
        if (!query) return;
        clearFilter();
        break;
      default:
        return;
    }
    e.preventDefault();
  }

  function onInputKey(e: KeyboardEvent): void {
    if (isComposing(e)) return;
    if (e.code === 'Escape') {
      e.preventDefault();
      if (filter) clearFilter();
      else list?.focus();
    } else if (e.code === 'ArrowDown' || e.code === 'Enter') {
      e.preventDefault();
      if (rows.length === 0) return;
      if (selIndex < 0) select(rows[0].e, true);
      list?.focus();
      scrollToSel();
    }
  }

  function blame(path: string): void {
    if (rev) repo.openView({ kind: 'blame', path, rev, cwd: null });
  }

  function history(path: string): void {
    if (rev) repo.openView({ kind: 'history', path, rev });
  }

  function copy(text: string): void {
    void copyText(text).then(() => toast('Copied'));
  }

  function rowMenu(ev: MouseEvent, r: Row): void {
    const e = r.e;
    select(e, true);
    const items: MenuEntry[] = [{ label: 'Copy path', icon: Copy, action: () => copy(e.path) }];
    if (e.kind === 'blob') {
      items.push(
        { separator: true },
        { label: 'Blame', icon: ScanText, action: () => blame(e.path) },
        { label: 'History', icon: RotateCcwClock, action: () => history(e.path) },
      );
    } else if (e.kind === 'commit') {
      items.push({ label: 'Copy SHA', icon: Hash, action: () => copy(e.oid) });
    }
    showContextMenu(ev, items);
  }

  function revealRev(): void {
    if (rev && /^[0-9a-f]{40,64}$/i.test(rev)) void repo.reveal(rev);
  }

  function onCrumb(path: string): void {
    void revealPath(path, navSeq);
    if (!isEditable(document.activeElement)) list?.focus();
  }
</script>

<div class="tv">
  <div class="panel-header bar">
    <button class="icon-btn" title="Back (Esc)" onclick={() => repo.back()}>
      <ArrowLeft size={14} strokeWidth={1.75} />
    </button>
    <span class="mode" title="Files"><FolderTree size={14} strokeWidth={1.75} /></span>
    {#if rev}
      <button class="rev" title={rev} onclick={revealRev}>{shortRev(rev)}</button>
    {/if}
    <span class="crumbs">
      {#each crumbs as c, i (c.path)}
        <span class="slash">/</span>
        <button class="crumb" class:last={i === crumbs.length - 1} title={c.path} onclick={() => onCrumb(c.path)}>
          {c.name}
        </button>
      {/each}
    </span>
    <span class="grow"></span>
    {#if shown?.kind === 'blob'}
      {@const f = shown}
      <button class="icon-btn" title="Blame" onclick={() => blame(f.path)}>
        <ScanText size={14} strokeWidth={1.75} />
      </button>
      <button class="icon-btn" title="History" onclick={() => history(f.path)}>
        <RotateCcwClock size={14} strokeWidth={1.75} />
      </button>
    {/if}
  </div>
  <div class="body">
    {#if switching}<div class="progress"></div>{/if}
    <div class="side">
      <div class="filter">
        <span class="ficon"><Search size={12} strokeWidth={1.75} /></span>
        <input
          class="input"
          bind:this={input}
          bind:value={filter}
          spellcheck="false"
          autocomplete="off"
          autocorrect="off"
          autocapitalize="off"
          onkeydown={onInputKey}
        />
        {#if allRev}
          <span class="fend"><Spinner size={10} /></span>
        {:else if filter}
          <button class="fend clear" title="Clear (Esc)" onclick={clearFilter}><X size={12} strokeWidth={1.75} /></button>
        {/if}
      </div>
      <div class="list" class:stale={switching}>
        {#if rootError}
          <div class="ph empty"><span class="msg selectable">{rootError}</span></div>
        {:else if query && allError}
          <div class="ph empty"><span class="msg selectable">{allError}</span></div>
        {:else}
          <VirtualList bind:this={list} count={rows.length} rowHeight={ROW_H} tabindex={0} onkeydown={onKey}>
            {#snippet row(i)}
              {@const r = rows[i]}
              {#if r}
                {@const e = r.e}
                {@const open = e.kind === 'tree' && expanded.has(e.path)}
                <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
                <div
                  class="row"
                  class:sel={e.path === selPath}
                  style:padding-left="{6 + r.depth * INDENT}px"
                  title={e.path}
                  onclick={() => clickRow(r)}
                  ondblclick={() => {
                    if (query) clearFilter();
                  }}
                  oncontextmenu={(ev) => rowMenu(ev, r)}
                >
                  {#if query}
                    <span class="ic file">
                      {#if e.kind === 'commit'}
                        <Boxes size={13} strokeWidth={1.5} />
                      {:else}
                        <FileIcon size={13} strokeWidth={1.5} />
                      {/if}
                    </span>
                    <span class="name">{basename(e.path)}</span>
                    <span class="where">{dirname(e.path)}</span>
                  {:else}
                    <span class="chev" class:open>
                      {#if e.kind === 'tree'}
                        {#if busy.has(e.path)}
                          <Spinner size={9} />
                        {:else if failed.has(e.path)}
                          <span class="warn" title={failed.get(e.path)}>
                            <TriangleExclamationPoint size={11} strokeWidth={1.75} />
                          </span>
                        {:else}
                          <ChevronRight size={12} strokeWidth={1.75} />
                        {/if}
                      {/if}
                    </span>
                    {#if e.kind === 'tree'}
                      <span class="ic dir">
                        {#if open}
                          <FolderOpen size={13} strokeWidth={1.5} />
                        {:else}
                          <Folder size={13} strokeWidth={1.5} />
                        {/if}
                      </span>
                    {:else}
                      <span class="ic file">
                        {#if e.kind === 'commit'}
                          <Boxes size={13} strokeWidth={1.5} />
                        {:else if e.mode === '120000'}
                          <FileSymlink size={13} strokeWidth={1.5} />
                        {:else}
                          <FileIcon size={13} strokeWidth={1.5} />
                        {/if}
                      </span>
                    {/if}
                    <span class="name">{basename(e.path)}</span>
                  {/if}
                </div>
              {/if}
            {/snippet}
          </VirtualList>
        {/if}
      </div>
    </div>
    <div class="main">
      {#if shown?.kind === 'blob' && spec}
        <div class="pane"><FileViewer bind:this={viewer} {spec} path={shown.path} /></div>
      {:else if shown?.kind === 'commit'}
        <div class="ph empty">
          <Boxes size={28} strokeWidth={1.5} />
          <span class="sub selectable" title={shown.oid}>{shortOid(shown.oid)}</span>
        </div>
      {/if}
    </div>
  </div>
</div>

<style>
  .tv {
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
  .crumbs {
    display: flex;
    align-items: center;
    min-width: 0;
    overflow: hidden;
    font-size: var(--fs-sm);
    white-space: nowrap;
  }
  .slash {
    flex: none;
    padding: 0 3px;
    color: var(--text-3);
  }
  .crumb {
    flex: 0 1 auto;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    color: var(--text-2);
  }
  .crumb:hover {
    color: var(--text-0);
  }
  .crumb.last {
    flex: none;
    max-width: 100%;
    color: var(--text-0);
  }
  .body {
    position: relative;
    flex: 1;
    min-height: 0;
    display: flex;
  }
  .side {
    display: flex;
    flex-direction: column;
    flex: none;
    width: 280px;
    border-right: 1px solid var(--border);
  }
  .filter {
    position: relative;
    flex: none;
    display: flex;
    align-items: center;
    padding: 6px;
  }
  .filter .input {
    width: 100%;
    padding-left: 24px;
    padding-right: 24px;
  }
  .ficon {
    position: absolute;
    left: 14px;
    display: inline-flex;
    color: var(--text-3);
    pointer-events: none;
  }
  .fend {
    position: absolute;
    right: 10px;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 18px;
    height: 18px;
    color: var(--text-2);
  }
  .clear {
    border-radius: var(--radius-sm);
  }
  .clear:hover {
    color: var(--text-0);
    background: var(--bg-hover);
  }
  .list {
    position: relative;
    flex: 1;
    min-height: 0;
  }
  .main {
    position: relative;
    flex: 1;
    min-width: 0;
  }
  .pane {
    position: absolute;
    inset: 0;
  }
  .row {
    display: flex;
    align-items: center;
    gap: 4px;
    height: 100%;
    padding-right: 8px;
    font-size: var(--fs-sm);
    color: var(--text-0);
    white-space: nowrap;
  }
  .row:hover {
    background: var(--bg-hover);
  }
  .row.sel {
    background: var(--bg-active);
  }
  .list:focus-within .row.sel {
    background: var(--bg-active-strong);
  }
  .chev {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    flex: none;
    width: 12px;
    color: var(--text-2);
  }
  /* Only the chevron itself (a direct child), not the warning icon of a failed folder. */
  .chev.open > :global(svg) {
    transform: rotate(90deg);
  }
  .warn {
    display: inline-flex;
    color: var(--st-conflict);
  }
  .ic {
    display: inline-flex;
    flex: none;
  }
  .ic.dir {
    color: var(--text-2);
  }
  .ic.file {
    color: var(--text-3);
  }
  .name {
    flex: 0 1 auto;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .where {
    flex: 1 1 0;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    color: var(--text-2);
    font-size: var(--fs-xs);
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
    color: var(--text-2);
    font-size: var(--fs-xs);
    text-align: center;
    white-space: pre-wrap;
    word-break: break-word;
  }
  .sub {
    color: var(--text-2);
    font-family: var(--font-mono);
    font-size: var(--fs-xs);
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
