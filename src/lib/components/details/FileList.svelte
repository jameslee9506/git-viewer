<!--
  Changed files of one diff target, as a sorted list or a folder tree (settings.fileListMode).
  Opening a file shows its diff with this list (in display order) as siblings, or calls `onopen`.
  Virtualized above 200 files. Module exports: `modeSeg` (list/tree toggle), `filterBox` (path filter
  input), `fileBar` (count · +a −d · filter · extra · toggle · trailing, the toolbar shared by the
  details views), `noMatch`, and `fileFilter(repo)`: the path filter of a repo tab, one query for all
  its details views that survives selection changes. The views filter the entries they pass in.
-->
<script module lang="ts">
  import Files from '@lucide/svelte/icons/files';
  import List from '@lucide/svelte/icons/list';
  import ListTree from '@lucide/svelte/icons/list-tree';
  import Search from '@lucide/svelte/icons/search';
  import SearchX from '@lucide/svelte/icons/search-x';
  import X from '@lucide/svelte/icons/x';
  import type { Snippet } from 'svelte';
  import type { RepoStore } from '../../state/repo.svelte';
  import { settings } from '../../state/settings.svelte';
  import { isComposing } from '../../util/keys';
  import { queryTerms } from '../sidebar/tree';

  /**
   * Path filter of the changed-file lists of a repo tab (see `fileFilter`). Getters, not `$derived`:
   * the instance outlives the view that created it, and a derived whose parent effect is destroyed
   * stops recomputing (it returns its stale value).
   */
  export class FileFilter {
    query = $state('');
    /** Lower-cased terms; [] = no filter. */
    get terms(): string[] {
      return queryTerms(this.query);
    }
    /** Normalized query ('' = no filter): FileList's `filter`. */
    get key(): string {
      return this.terms.join(' ');
    }
  }

  /** A file list as the filter box drives it (↓ focuses it, Enter opens its first file). */
  export interface FileListNav {
    focusList(): void;
    openFirst(): void;
  }

  /** What `fileBar` shows. */
  export interface FileBarData {
    /** Changed files, and how many of them the filter shows. */
    count: number;
    shown: number;
    additions: number;
    deletions: number;
    filter: FileFilter;
    /** The first file list on screen. */
    list: () => FileListNav | null | undefined;
  }

  const filters = new WeakMap<RepoStore, FileFilter>();

  /** The file filter of a repo tab: shared by its details views, kept while the selection changes. */
  export function fileFilter(repo: RepoStore): FileFilter {
    let f = filters.get(repo);
    if (!f) {
      f = new FileFilter();
      filters.set(repo, f);
    }
    return f;
  }

  /** Esc clears the query (then leaves the box), ↓ moves into the list, Enter opens the first match. */
  function onFilterKey(e: KeyboardEvent, f: FileFilter, list: () => FileListNav | null | undefined): void {
    if (isComposing(e) || e.metaKey || e.ctrlKey || e.altKey || e.shiftKey) return;
    switch (e.code) {
      case 'Escape':
        if (f.query) f.query = '';
        else if (e.currentTarget instanceof HTMLElement) e.currentTarget.blur();
        break;
      case 'ArrowDown':
      case 'Enter':
      case 'NumpadEnter': {
        const l = list();
        if (!l) return;
        if (e.code === 'ArrowDown') l.focusList();
        else l.openFirst();
        break;
      }
      default:
        return;
    }
    e.preventDefault();
    e.stopPropagation();
  }

  export { fileBar, filterBox, modeSeg, noMatch };
</script>

<script lang="ts">
  import Binary from '@lucide/svelte/icons/binary';
  import ChevronDown from '@lucide/svelte/icons/chevron-down';
  import ChevronRight from '@lucide/svelte/icons/chevron-right';
  import Copy from '@lucide/svelte/icons/copy';
  import FolderOpen from '@lucide/svelte/icons/folder-open';
  import FolderTree from '@lucide/svelte/icons/folder-tree';
  import RotateCcwClock from '@lucide/svelte/icons/rotate-ccw-clock';
  import ScanText from '@lucide/svelte/icons/scan-text';
  import SquareArrowOutUpRight from '@lucide/svelte/icons/square-arrow-out-up-right';
  import { openPath, revealItemInDir } from '@tauri-apps/plugin-opener';
  import { tick, untrack } from 'svelte';
  import { errorMessage } from '../../api';
  import { activeDiffKey, getRepo } from '../../state/repo.svelte';
  import { showContextMenu, toast } from '../../state/ui.svelte';
  import type { MenuEntry } from '../../state/ui.svelte';
  import { copyText } from '../../util/format';
  import VirtualList from '../common/VirtualList.svelte';
  import {
    blameView,
    entryKeys,
    expandedTo,
    fileLabel,
    fileRows,
    fileTitle,
    historyFor,
    joinPath,
    statusClass,
    treeView,
    visibleRows,
    worktreeCwd,
  } from './filetree';
  import type { FileEntry, FileRow } from './filetree';

  interface Props {
    entries: FileEntry[];
    /** Replaces the default open (repo.openDiff over this list in display order); index into entries. */
    onopen?: (index: number) => void;
    /**
     * The active path filter (`FileFilter.key`; '' = none) the entries were filtered by. While set,
     * folders collapse in a separate state that starts expanded for every query, so collapsed folders
     * never hide matches and the unfiltered state comes back with the full list.
     */
    filter?: string;
  }

  let { entries, onopen, filter = '' }: Props = $props();

  const ROW_H = 22;
  const VIRTUAL_MIN = 200;
  const repo = getRepo();

  const mode = $derived(settings.fileListMode);
  const keys = $derived(entryKeys(entries));
  const keyIndex = $derived(new Map(keys.map((k, i) => [k, i])));
  const allRows = $derived(fileRows(entries, mode));
  let collapsed = $state.raw<ReadonlySet<string>>(new Set());
  /** Folders collapsed under the current filter (reset whenever the query changes). */
  let filterCollapsed = $derived.by((): ReadonlySet<string> => {
    void filter;
    return new Set();
  });
  /** The collapsed folders in effect. */
  const shut = $derived(filter ? filterCollapsed : collapsed);
  const rows = $derived(mode === 'tree' ? visibleRows(allRows, shut) : allRows);
  /** By all rows (folders included), so collapsing folders never swaps the list element. */
  const virtual = $derived(allRows.length > VIRTUAL_MIN);
  const minHeight = $derived(Math.min(rows.length, 3) * ROW_H);

  /** Row of every entry in `rows`, -1 when hidden inside a collapsed folder. */
  const rowOf = $derived.by(() => {
    const m = new Int32Array(entries.length).fill(-1);
    rows.forEach((r, i) => {
      if (r.kind === 'file') m[r.index] = i;
    });
    return m;
  });

  const activeKey = $derived(activeDiffKey(repo.view));
  const activeEntry = $derived(activeKey == null ? -1 : (keyIndex.get(activeKey) ?? -1));
  const activeRow = $derived(activeEntry < 0 ? -1 : rowOf[activeEntry]);

  /** Keyboard cursor, kept by row identity so it survives refreshes and list/tree switches. */
  let cursorKey = $state<string | null>(null);
  const cursorRow = $derived.by(() => {
    const k = cursorKey;
    if (k == null) return -1;
    if (k.startsWith('f\0')) {
      const e = keyIndex.get(k.slice(2));
      return e == null ? -1 : rowOf[e];
    }
    const path = k.slice(2);
    return rows.findIndex((r) => r.kind === 'dir' && r.path === path);
  });
  let focused = $state(false);

  let root = $state.raw<HTMLDivElement>();
  let vl = $state.raw<ReturnType<typeof VirtualList>>();

  function rowKey(r: FileRow): string {
    return r.kind === 'dir' ? `d\0${r.path}` : `f\0${keys[r.index]}`;
  }

  // Follow the open diff (also when it is stepped from the viewer): move the cursor to it, reveal it.
  let followed: string | null = null;
  $effect(() => {
    const k = activeKey;
    const e = activeEntry;
    const r = activeRow;
    if (k === followed) return;
    if (k == null || e < 0) {
      followed = k;
      return;
    }
    if (r < 0) {
      untrack(() => {
        const next = expandedTo(shut, entries[e].item.file.path);
        if (next !== shut) setShut(next);
        else followed = k;
      });
      return;
    }
    followed = k;
    untrack(() => {
      cursorKey = `f\0${k}`;
      scrollToRow(r);
    });
  });

  // Crossing VIRTUAL_MIN rows swaps the list element (the filter cleared from its × while the list has
  // the focus — the button keeps focus where it is —, a refresh adding files, …) and drops a keyboard
  // focus it held: hand that focus to the new element.
  let refocus = false;
  $effect.pre(() => {
    void virtual;
    untrack(() => {
      // Runs before the swap: the outgoing element is still `root`.
      refocus = !!root && root.contains(document.activeElement);
    });
  });
  $effect(() => {
    void virtual;
    untrack(() => {
      if (!refocus) return;
      refocus = false;
      // A removed element need not report focusout.
      focused = false;
      // Once the new element (and VirtualList) are bound; only if nothing else took the focus meanwhile.
      void tick().then(() => {
        const a = document.activeElement;
        if (!a || a === document.body) focusList();
      });
    });
  });

  // ⌘C is a native menu accelerator: it never arrives as keydown, only as clipboard events, which
  // WebKit dispatches at the selection (or body), so they are caught at the document. Cancelling
  // `beforecopy` keeps Edit ▸ Copy enabled without a text selection.
  $effect(() => {
    const owns = () => {
      const sel = document.getSelection();
      if (sel && !sel.isCollapsed && sel.toString() !== '') return false;
      return !!root && root.contains(document.activeElement);
    };
    const onBeforeCopy = (e: Event) => {
      if (owns()) e.preventDefault();
    };
    const onCopy = (e: ClipboardEvent) => {
      if (!owns() || !e.clipboardData) return;
      const row = rows[cursorRow >= 0 ? cursorRow : activeRow];
      if (!row) return;
      e.clipboardData.setData('text/plain', row.kind === 'dir' ? row.path : entries[row.index].item.file.path);
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

  function open(index: number): void {
    if (index === activeEntry) return;
    if (onopen) {
      onopen(index);
      return;
    }
    const order: number[] = [];
    for (const r of allRows) if (r.kind === 'file') order.push(r.index);
    repo.openDiff(
      order.map((k) => entries[k].item),
      order.indexOf(index),
    );
  }

  function setShut(next: ReadonlySet<string>): void {
    if (filter) filterCollapsed = next;
    else collapsed = next;
  }

  function setCollapsed(path: string, closed: boolean): void {
    if (shut.has(path) === closed) return;
    const next = new Set(shut);
    if (closed) next.add(path);
    else next.delete(path);
    setShut(next);
  }

  /** Keyboard focus from outside (the filter's ↓): the cursor row, else the open file's or the first row. */
  export function focusList(): void {
    if (rows.length === 0) return;
    let i = cursorRow;
    if (i < 0) {
      i = activeRow >= 0 ? activeRow : 0;
      cursorKey = rowKey(rows[i]);
    }
    scrollToRow(i);
    if (virtual) vl?.focus();
    else root?.focus();
  }

  /** Open the first file shown (the filter's Enter). */
  export function openFirst(): void {
    for (const r of rows) {
      if (r.kind === 'file') {
        open(r.index);
        return;
      }
    }
  }

  function pageSize(): number {
    if (virtual) return vl?.pageSize() ?? 1;
    return Math.max(1, Math.floor((root?.clientHeight ?? 0) / ROW_H));
  }

  function scrollToRow(i: number): void {
    if (virtual) {
      vl?.scrollToIndex(i);
      return;
    }
    const el = root;
    if (!el) return;
    const top = i * ROW_H;
    if (top < el.scrollTop) el.scrollTop = top;
    else if (top + ROW_H > el.scrollTop + el.clientHeight) el.scrollTop = top + ROW_H - el.clientHeight;
  }

  /** Move the cursor; while a diff is open the diff follows onto files. */
  function moveTo(i: number): void {
    const row = rows[i];
    if (!row) return;
    cursorKey = rowKey(row);
    scrollToRow(i);
    if (row.kind === 'file' && repo.view.kind === 'diff') open(row.index);
  }

  /** Nearest folder row above row i that contains it (tree order), or -1. */
  function parentRow(i: number): number {
    const depth = rows[i].depth;
    for (let j = i - 1; j >= 0; j--) if (rows[j].depth < depth) return j;
    return -1;
  }

  function onkeydown(e: KeyboardEvent): void {
    if (e.isComposing || e.keyCode === 229 || rows.length === 0) return;
    const last = rows.length - 1;
    const cur = cursorRow >= 0 ? cursorRow : activeRow;
    const row = cur >= 0 ? rows[cur] : undefined;
    const bare = !e.metaKey && !e.ctrlKey && !e.altKey && !e.shiftKey;
    const step = (d: 1 | -1) => (cur < 0 ? 0 : Math.max(0, Math.min(last, cur + d)));
    let to = -1;
    if ((e.key === 'ArrowDown' || e.key === 'ArrowUp') && !e.altKey && !e.ctrlKey && !e.shiftKey) {
      const down = e.key === 'ArrowDown';
      to = e.metaKey ? (down ? last : 0) : step(down ? 1 : -1);
    } else if (bare && (e.code === 'KeyJ' || e.code === 'KeyK')) {
      to = step(e.code === 'KeyJ' ? 1 : -1);
    } else if (bare && e.key === 'PageDown') {
      to = Math.min(last, Math.max(cur, 0) + pageSize());
    } else if (bare && e.key === 'PageUp') {
      to = Math.max(0, cur - pageSize());
    } else if (bare && e.key === 'Home') {
      to = 0;
    } else if (bare && e.key === 'End') {
      to = last;
    } else if (bare && e.key === 'Enter' && row) {
      if (row.kind === 'dir') setCollapsed(row.path, !shut.has(row.path));
      else open(row.index);
    } else if (bare && e.key === 'ArrowRight' && row) {
      if (row.kind === 'file') open(row.index);
      else if (shut.has(row.path)) setCollapsed(row.path, false);
      else to = Math.min(last, cur + 1);
    } else if (bare && e.key === 'ArrowLeft' && row && mode === 'tree') {
      if (row.kind === 'dir' && !shut.has(row.path)) setCollapsed(row.path, true);
      else {
        to = parentRow(cur);
        if (to < 0) return;
      }
    } else {
      return;
    }
    e.preventDefault();
    e.stopPropagation();
    if (to >= 0) moveTo(to);
  }

  function rowAt(e: Event): number {
    const el = e.target instanceof Element ? e.target.closest<HTMLElement>('[data-row]') : null;
    return el ? Number(el.dataset.row) : -1;
  }

  function onclick(e: MouseEvent): void {
    const row = rows[rowAt(e)];
    if (!row) return;
    cursorKey = rowKey(row);
    if (row.kind === 'dir') setCollapsed(row.path, !shut.has(row.path));
    else open(row.index);
  }

  function oncontextmenu(e: MouseEvent): void {
    const row = rows[rowAt(e)];
    if (!row) return;
    cursorKey = rowKey(row);
    showContextMenu(e, row.kind === 'dir' ? dirMenu(row.path) : fileMenu(entries[row.index]));
  }

  function copy(text: string): void {
    void copyText(text).then(() => toast('Copied'));
  }

  function reveal(path: string): void {
    revealItemInDir(path).catch((err: unknown) => toast(errorMessage(err)));
  }

  /** Open a working-tree file (a folder: in Finder) with its default app. */
  function openFile(path: string): void {
    openPath(path).catch((err: unknown) => toast(errorMessage(err)));
  }

  function fileMenu(entry: FileEntry): MenuEntry[] {
    const { item } = entry;
    const { path, status } = item.file;
    const cwd = worktreeCwd(item.target);
    const blame = blameView(entry);
    const history = historyFor(item, repo.id, repo.refs);
    const tree = treeView(item);
    const menu: MenuEntry[] = [{ label: 'Copy Path', icon: Copy, hint: '⌘C', action: () => copy(path) }];
    if (cwd != null) menu.push({ label: 'Copy Full Path', icon: Copy, action: () => copy(joinPath(cwd, path)) });
    menu.push(
      { separator: true },
      { label: 'Blame', icon: ScanText, disabled: !blame, action: () => blame && repo.openView(blame) },
      {
        label: 'File History',
        icon: RotateCcwClock,
        disabled: !history,
        action: () => history && repo.openView(history),
      },
    );
    if (tree) menu.push({ label: 'Open at Revision', icon: FolderTree, action: () => repo.openView(tree) });
    if (cwd != null) {
      const full = joinPath(cwd, path);
      const gone = status === 'D';
      menu.push(
        { separator: true },
        { label: 'Open in Default App', icon: SquareArrowOutUpRight, disabled: gone, action: () => openFile(full) },
        { label: 'Reveal in Finder', icon: FolderOpen, disabled: gone, action: () => reveal(full) },
      );
    }
    return menu;
  }

  function dirMenu(path: string): MenuEntry[] {
    const menu: MenuEntry[] = [{ label: 'Copy Path', icon: Copy, hint: '⌘C', action: () => copy(path) }];
    const cwd = entries.length > 0 ? worktreeCwd(entries[0].item.target) : null;
    if (cwd != null) {
      menu.push(
        { label: 'Copy Full Path', icon: Copy, action: () => copy(joinPath(cwd, path)) },
        { separator: true },
        { label: 'Reveal in Finder', icon: FolderOpen, action: () => reveal(joinPath(cwd, path)) },
      );
    }
    return menu;
  }

  function indent(depth: number): number {
    return 6 + depth * 12;
  }
</script>

{#snippet modeSeg()}
  <div class="seg">
    <button
      class:on={settings.fileListMode === 'list'}
      title="List"
      onclick={() => (settings.fileListMode = 'list')}
    >
      <List size={13} strokeWidth={1.75} />
    </button>
    <button
      class:on={settings.fileListMode === 'tree'}
      title="Tree"
      onclick={() => (settings.fileListMode = 'tree')}
    >
      <ListTree size={13} strokeWidth={1.75} />
    </button>
  </div>
{/snippet}

{#snippet filterBox(f: FileFilter, list: () => FileListNav | null | undefined)}
  <span class="ffilter" class:q={f.query !== ''}>
    <span class="ficon"><Search size={12} strokeWidth={1.75} /></span>
    <input
      class="input"
      value={f.query}
      spellcheck="false"
      autocomplete="off"
      autocorrect="off"
      autocapitalize="off"
      aria-label="Filter files"
      oninput={(e) => (f.query = e.currentTarget.value)}
      onkeydown={(e) => onFilterKey(e, f, list)}
    />
    {#if f.query}
      <!-- mousedown cancelled: the input keeps the focus -->
      <button
        class="fclear"
        tabindex="-1"
        title="Clear"
        onmousedown={(e) => e.preventDefault()}
        onclick={() => (f.query = '')}
      >
        <X size={12} strokeWidth={1.75} />
      </button>
    {/if}
  </span>
{/snippet}

{#snippet noMatch()}
  <div class="nomatch"><SearchX size={16} strokeWidth={1.75} /></div>
{/snippet}

{#snippet fileBar(bar: FileBarData, extra?: Snippet, trailing?: Snippet)}
  {@const filtering = bar.filter.key !== ''}
  <div class="fbar">
    <span class="fcount" title={filtering ? 'Matching / changed files' : 'Changed files'}>
      <Files size={13} strokeWidth={1.75} /><span class="num">{filtering ? `${bar.shown}/${bar.count}` : bar.count}</span>
    </span>
    {#if bar.additions > 0}<span class="num add">+{bar.additions}</span>{/if}
    {#if bar.deletions > 0}<span class="num del">−{bar.deletions}</span>{/if}
    {@render filterBox(bar.filter, bar.list)}
    {@render extra?.()}
    {@render modeSeg()}
    {@render trailing?.()}
  </div>
{/snippet}

{#snippet rowView(row: FileRow, i: number)}
  {#if row.kind === 'dir'}
    {@const closed = shut.has(row.path)}
    <div
      class="row folder"
      class:cursor={focused && i === cursorRow}
      role="treeitem"
      aria-selected="false"
      aria-expanded={!closed}
      aria-level={row.depth + 1}
      data-row={i}
      style:padding-left="{indent(row.depth)}px"
      title={row.path}
    >
      <span class="chev">
        {#if closed}
          <ChevronRight size={12} strokeWidth={1.75} />
        {:else}
          <ChevronDown size={12} strokeWidth={1.75} />
        {/if}
      </span>
      <span class="name ellipsis">{row.name}</span>
    </div>
  {:else}
    {@const entry = entries[row.index]}
    {@const file = entry.item.file}
    {@const label = fileLabel(file)}
    {@const loc = mode === 'tree' ? (label.fromDir == null ? '' : `← ${label.fromDir}`) : label.dir}
    <div
      class="row"
      class:active={i === activeRow}
      class:cursor={focused && i === cursorRow}
      role="treeitem"
      aria-selected={i === activeRow}
      aria-level={row.depth + 1}
      data-row={i}
      style:padding-left="{mode === 'tree' ? indent(row.depth) : 8}px"
      title={fileTitle(file)}
    >
      <span class="st {statusClass(file.status)}">{file.status}</span>
      <span class="name ellipsis">{label.name}</span>
      {#if loc}<span class="loc ellipsis">{loc}</span>{/if}
      <span class="stat">
        {#if entry.binary}
          <Binary size={12} strokeWidth={1.75} />
        {:else}
          {#if entry.additions}<span>+{entry.additions}</span>{/if}
          {#if entry.deletions}<span>−{entry.deletions}</span>{/if}
        {/if}
      </span>
    </div>
  {/if}
{/snippet}

{#if virtual}
  <div
    bind:this={root}
    class="fl"
    class:focused
    role="tree"
    tabindex="-1"
    style:height="{rows.length * ROW_H}px"
    style:min-height="{minHeight}px"
    {onkeydown}
    {onclick}
    {oncontextmenu}
    onfocusin={() => (focused = true)}
    onfocusout={() => (focused = false)}
  >
    <VirtualList bind:this={vl} count={rows.length} rowHeight={ROW_H} overscan={12} tabindex={0}>
      {#snippet row(i)}
        {@render rowView(rows[i], i)}
      {/snippet}
    </VirtualList>
  </div>
{:else}
  <div
    bind:this={root}
    class="fl scroll"
    class:focused
    role="tree"
    tabindex="0"
    style:min-height="{minHeight}px"
    {onkeydown}
    {onclick}
    {oncontextmenu}
    onfocusin={() => (focused = true)}
    onfocusout={() => (focused = false)}
  >
    {#each rows as r, i}
      {@render rowView(r, i)}
    {/each}
  </div>
{/if}

<style>
  .fl {
    position: relative;
    flex: 0 1 auto;
    min-width: 0;
    overflow: hidden;
    font-size: var(--fs-sm);
  }
  .fl.scroll {
    overflow-y: auto;
  }
  .row {
    display: flex;
    align-items: center;
    gap: 6px;
    height: 22px;
    padding-right: 8px;
    color: var(--text-0);
    white-space: nowrap;
  }
  .row:hover {
    background: var(--bg-hover);
  }
  .row.active {
    background: var(--bg-active);
  }
  .row.cursor {
    background: var(--bg-active-strong);
  }
  .folder {
    gap: 4px;
    color: var(--text-1);
  }
  .chev {
    display: inline-flex;
    flex: none;
    color: var(--text-2);
  }
  .name {
    flex: 0 1 auto;
  }
  .loc {
    flex: 1 1 0;
    color: var(--text-2);
    font-size: var(--fs-xs);
  }
  .stat {
    flex: none;
    margin-left: auto;
    display: inline-flex;
    align-items: center;
    gap: 4px;
    color: var(--text-2);
    font-family: var(--font-mono);
    font-size: var(--fs-xs);
  }
  .fbar {
    flex: none;
    display: flex;
    align-items: center;
    gap: 8px;
    height: 30px;
    padding: 0 6px 0 12px;
    border-top: 1px solid var(--border);
    border-bottom: 1px solid var(--border);
    color: var(--text-2);
    font-size: var(--fs-xs);
  }
  .fcount {
    display: inline-flex;
    align-items: center;
    gap: 4px;
  }
  .num {
    font-family: var(--font-mono);
    font-size: var(--fs-xs);
  }
  /* Takes the free space of its bar (up to max-width); the auto margin pushes what follows right. */
  .ffilter {
    position: relative;
    flex: 1 1 0;
    display: flex;
    min-width: 48px;
    max-width: 220px;
    margin-right: auto;
  }
  .ffilter .input {
    flex: 1 1 auto;
    width: 100%;
    height: 24px;
    padding: 0 6px 0 22px;
  }
  .ffilter.q .input {
    padding-right: 22px;
  }
  .ficon {
    position: absolute;
    top: 0;
    bottom: 0;
    left: 7px;
    display: flex;
    align-items: center;
    color: var(--text-3);
    pointer-events: none;
  }
  .ffilter:focus-within .ficon {
    color: var(--text-2);
  }
  .fclear {
    position: absolute;
    top: 3px;
    right: 3px;
    width: 18px;
    height: 18px;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    border-radius: var(--radius-sm);
    color: var(--text-2);
  }
  .fclear:hover {
    color: var(--text-0);
    background: var(--bg-hover);
  }
  .nomatch {
    flex: 1 1 auto;
    display: flex;
    align-items: center;
    justify-content: center;
    min-height: 44px;
    color: var(--text-3);
  }
</style>
