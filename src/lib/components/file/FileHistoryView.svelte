<!--
  File history (`repo.view` kind 'history'): commits touching the file, following renames, on the
  left (↑↓ / j k move, Enter selects the commit in the graph, f toggles diff/file, b blames); on the
  right the selected commit's diff of the file, or the file as of that commit.
-->
<script module lang="ts">
  /** Diff/file toggle, remembered across openings. */
  let lastMode: 'diff' | 'file' = 'diff';

  /**
   * Where a history was left (highlighted commit, list scrollTop), by its view object: a viewer opened
   * over it remembers that object and `repo.back()` brings the same object back to a new instance.
   */
  const leftAt = new WeakMap<object, { oid: string; top: number }>();
</script>

<script lang="ts">
  import ArrowLeft from '@lucide/svelte/icons/arrow-left';
  import ArrowRightLeft from '@lucide/svelte/icons/arrow-right-left';
  import Copy from '@lucide/svelte/icons/copy';
  import FileDiff from '@lucide/svelte/icons/file-diff';
  import FileText from '@lucide/svelte/icons/file-text';
  import FolderTree from '@lucide/svelte/icons/folder-tree';
  import Hash from '@lucide/svelte/icons/hash';
  import Locate from '@lucide/svelte/icons/locate';
  import RotateCcwClock from '@lucide/svelte/icons/rotate-ccw-clock';
  import ScanText from '@lucide/svelte/icons/scan-text';
  import TextQuote from '@lucide/svelte/icons/text-quote';
  import { onMount, tick, untrack } from 'svelte';
  import { api, errorMessage } from '../../api';
  import { getRepo } from '../../state/repo.svelte';
  import { settings } from '../../state/settings.svelte';
  import { showContextMenu, toast } from '../../state/ui.svelte';
  import type { BlobSpec, ChangeStatus, DiffTarget, HistoryEntry } from '../../types';
  import {
    absoluteDate,
    absoluteTime,
    basename,
    copyText,
    dirname,
    relativeTime,
    shortOid,
  } from '../../util/format';
  import VirtualList from '../common/VirtualList.svelte';
  import DiffContent from '../diff/DiffContent.svelte';
  import { isComposing, isEditable } from '../../util/keys';
  import FileViewer, { escapeBack } from './FileViewer.svelte';

  const ROW_H = 24;
  /** Entries fetched per history (api.fileHistory's default cap). */
  const LIMIT = 2000;
  /** Keyboard moves update the right side after this idle time (fast key repeat). */
  const FOLLOW_MS = 60;

  const repo = getRepo();
  let entries = $state.raw<HistoryEntry[]>([]);
  /** path + rev of the shown entries. */
  let shownKey = $state<string | null>(null);
  /** Highlighted row. */
  let sel = $state(0);
  /** Row whose diff / file is on the right (follows `sel` after FOLLOW_MS on key repeat). */
  let shown = $state(0);
  let mode = $state<'diff' | 'file'>(lastMode);
  let loading = $state(false);
  let error = $state<string | null>(null);
  let list: VirtualList | undefined = $state();
  let content: DiffContent | undefined = $state();
  let viewer: FileViewer | undefined = $state();
  let seq = 0;
  let timer: ReturnType<typeof setTimeout> | undefined;
  /** View object of the listed entries (see leftAt). */
  let shownView: object | null = null;
  /** The list's virtual scrollTop (kept here: on destroy the list is already detached). */
  let listTop = 0;

  const view = $derived(repo.view.kind === 'history' ? repo.view : null);
  /**
   * The commit HEAD points at (resolved in repo.id, where the history runs). A primitive: refs reloads
   * that leave HEAD in place (window focus, fetch, stash) do not notify, so they re-run no history.
   */
  const headOid = $derived(repo.refs?.head.oid ?? null);
  const wantKey = $derived(view ? histKey(view.path, view.rev) : null);
  const stale = $derived(loading && shownKey !== wantKey);
  const current = $derived(entries[sel] ?? null);
  const entry = $derived(entries[shown] ?? null);
  // Primitive deriveds keep the objects below stable when a refresh returns the same entry.
  const oid = $derived(entry?.oid ?? null);
  const fileRev = $derived(entry ? revisionOf(entry) : null);
  const filePath = $derived(entry?.path ?? null);
  const target = $derived<DiffTarget | null>(oid ? { kind: 'commit', oid, parent: null } : null);
  const fileSpec = $derived<BlobSpec | null>(
    fileRev && filePath ? { kind: 'rev', rev: fileRev, path: filePath } : null,
  );

  function histKey(path: string, rev: string | null): string {
    return `${path}\0${rev ?? ''}`;
  }

  /** Revision holding the file's content for an entry (a deletion shows the last version). */
  function revisionOf(e: HistoryEntry): string {
    return e.status === 'D' ? (e.parents[0] ?? `${e.oid}^`) : e.oid;
  }

  $effect(() => {
    const v = view;
    if (!v) return;
    // History from HEAD changes only when HEAD moves (other revisions are commits: fixed).
    if (v.rev == null) void headOid;
    untrack(() => void load(v));
  });

  onMount(() => {
    const off = escapeBack(repo);
    return () => {
      // Another viewer covering this one keeps its view object for back(): remember the place.
      const e = entries[sel];
      if (shownView && e) leftAt.set(shownView, { oid: e.oid, top: listTop });
      off();
      clearTimeout(timer);
      // Loads in flight drop their result and never retry (which would cancel a newer view's run).
      seq++;
    };
  });

  async function load(v: { path: string; rev: string | null }): Promise<void> {
    const { path, rev } = v;
    const k = histKey(path, rev);
    const id = ++seq;
    loading = true;
    try {
      const request = () => api.fileHistory(repo.id, path, rev, true, LIMIT);
      // The backend runs one history per repo: a newer request kills the older run ("cancelled").
      // Cancelled while still the newest (an older request reached the backend after this one): ask
      // once more.
      const next = await request().catch((e: unknown) => {
        if (id !== seq || errorMessage(e) !== 'cancelled') throw e;
        return request();
      });
      if (id !== seq) return;
      const same = k === shownKey;
      // A refresh keeps the highlighted commit, back() the one it was left at; a new file starts at the
      // selected commit, if listed.
      const left = same ? undefined : leftAt.get(v);
      const s = repo.selection;
      const keep = same ? entries[sel]?.oid : (left?.oid ?? (s.kind === 'commit' ? s.oid : undefined));
      const j = keep ? next.findIndex((e) => e.oid === keep) : -1;
      entries = next;
      shownKey = k;
      shownView = v;
      error = null;
      setSel(j >= 0 ? j : same ? Math.min(sel, Math.max(0, next.length - 1)) : 0, true);
      if (!same) {
        await tick();
        if (id !== seq) return;
        if (left && j >= 0) {
          list?.setScrollTop(left.top);
          // Still in view if commits were added above it meanwhile (a history from HEAD).
          list?.scrollToIndex(sel);
        } else list?.scrollToIndex(sel, sel > 0 ? 'center' : 'start');
        listTop = list?.getScrollTop() ?? 0;
        if (!isEditable(document.activeElement)) list?.focus();
      }
    } catch (e) {
      if (id !== seq) return;
      entries = [];
      shownKey = k;
      shownView = v;
      error = errorMessage(e);
      setSel(0, true);
    } finally {
      if (id === seq) loading = false;
    }
  }

  function setSel(i: number, immediate = false): void {
    sel = i;
    clearTimeout(timer);
    if (immediate) shown = i;
    else timer = setTimeout(() => (shown = sel), FOLLOW_MS);
  }

  function move(to: number): void {
    const n = entries.length;
    if (n === 0) return;
    const i = Math.max(0, Math.min(n - 1, to));
    setSel(i);
    list?.scrollToIndex(i);
  }

  function pick(i: number): void {
    setSel(i, true);
    list?.focus();
  }

  function setMode(m: 'diff' | 'file'): void {
    mode = m;
    lastMode = m;
  }

  function blame(e: HistoryEntry | null): void {
    if (e) repo.openView({ kind: 'blame', path: e.path, rev: revisionOf(e), cwd: null });
  }

  function copy(text: string): void {
    void copyText(text).then(() => toast('Copied'));
  }

  function menu(ev: MouseEvent, i: number): void {
    const e = entries[i];
    if (!e) return;
    setSel(i, true);
    showContextMenu(ev, [
      { label: 'Select commit', icon: Locate, hint: '↵', action: () => void repo.reveal(e.oid) },
      { label: 'Blame', icon: ScanText, hint: 'B', action: () => blame(e) },
      {
        label: 'Browse files',
        icon: FolderTree,
        action: () => repo.openView({ kind: 'tree', rev: e.oid, path: e.status === 'D' ? null : e.path }),
      },
      { separator: true },
      { label: 'Copy SHA', icon: Copy, action: () => copy(e.oid) },
      { label: 'Copy short SHA', icon: Hash, action: () => copy(shortOid(e.oid)) },
      { label: 'Copy subject', icon: TextQuote, action: () => copy(e.subject) },
    ]);
  }

  function onKey(e: KeyboardEvent): void {
    if (isComposing(e) || entries.length === 0) return;
    const plain = !e.metaKey && !e.ctrlKey && !e.altKey;
    const page = list?.pageSize() ?? 10;
    switch (e.code) {
      case 'ArrowDown':
        if (e.ctrlKey || e.shiftKey) return;
        if (e.altKey) content?.step(1);
        else move(e.metaKey ? entries.length - 1 : sel + 1);
        break;
      case 'ArrowUp':
        if (e.ctrlKey || e.shiftKey) return;
        if (e.altKey) content?.step(-1);
        else move(e.metaKey ? 0 : sel - 1);
        break;
      case 'KeyN':
        if (!plain || e.shiftKey) return;
        content?.step(1);
        break;
      case 'KeyP':
        if (!plain || e.shiftKey) return;
        content?.step(-1);
        break;
      case 'ArrowRight':
        if (!plain) return;
        if (mode === 'diff') content?.focus();
        else viewer?.focus();
        break;
      case 'KeyJ':
        if (!plain) return;
        move(sel + 1);
        break;
      case 'KeyK':
        if (!plain) return;
        move(sel - 1);
        break;
      case 'PageDown':
        move(sel + page);
        break;
      case 'PageUp':
        move(sel - page);
        break;
      case 'Home':
        move(0);
        break;
      case 'End':
        move(entries.length - 1);
        break;
      case 'Enter':
        if (current) void repo.reveal(current.oid);
        break;
      case 'KeyF':
        if (!plain) return;
        setMode(mode === 'diff' ? 'file' : 'diff');
        break;
      case 'KeyB':
        if (!plain) return;
        blame(current);
        break;
      default:
        return;
    }
    e.preventDefault();
  }

  function stClass(s: ChangeStatus): string {
    return s === '?' ? 'q' : s;
  }

  function dateText(t: number): string {
    return settings.dateFormat === 'relative' ? relativeTime(t) : absoluteDate(t);
  }

  function rowTip(e: HistoryEntry): string {
    return `${shortOid(e.oid)} ${e.subject}\n${e.author} <${e.email}>\n${absoluteTime(e.time)}`;
  }
</script>

{#snippet pathText(p: string)}
  {@const dir = dirname(p)}
  {#if dir}<span class="dir">{dir}/</span>{/if}<span class="name">{basename(p)}</span>
{/snippet}

<div class="hv">
  <div class="panel-header bar">
    <button class="icon-btn" title="Back (Esc)" onclick={() => repo.back()}>
      <ArrowLeft size={14} strokeWidth={1.75} />
    </button>
    <span class="mode" title="History"><RotateCcwClock size={14} strokeWidth={1.75} /></span>
    {#if current}
      <span class="st st-{stClass(current.status)}">{current.status}</span>
      <span class="path selectable" title={current.oldPath ? `${current.oldPath} → ${current.path}` : current.path}>
        {#if current.oldPath}{@render pathText(current.oldPath)}<span class="arrow">→</span>{/if}{@render pathText(current.path)}
      </span>
    {:else if view}
      <span class="path selectable" title={view.path}>{@render pathText(view.path)}</span>
    {/if}
    <span class="grow"></span>
    {#if entries.length > 0}
      <span class="count">{sel + 1}/{entries.length}{entries.length >= LIMIT ? '+' : ''}</span>
    {/if}
    <button class="icon-btn" title="Blame (B)" disabled={!current} onclick={() => blame(current)}>
      <ScanText size={14} strokeWidth={1.75} />
    </button>
    <div class="seg">
      <button class:on={mode === 'diff'} title="Diff (F)" onclick={() => setMode('diff')}>
        <FileDiff size={13} strokeWidth={1.75} />
      </button>
      <button class:on={mode === 'file'} title="File (F)" onclick={() => setMode('file')}>
        <FileText size={13} strokeWidth={1.75} />
      </button>
    </div>
  </div>
  <div class="body">
    {#if loading}<div class="progress"></div>{/if}
    <div class="list" class:stale>
      {#if entries.length > 0}
        <VirtualList
          bind:this={list}
          count={entries.length}
          rowHeight={ROW_H}
          tabindex={0}
          onkeydown={onKey}
          onscroll={(t) => (listTop = t)}
        >
          {#snippet row(i)}
            {@const e = entries[i]}
            {#if e}
              <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
              <div
                class="row"
                class:sel={i === sel}
                title={rowTip(e)}
                onclick={() => pick(i)}
                ondblclick={() => void repo.reveal(e.oid)}
                oncontextmenu={(ev) => menu(ev, i)}
              >
                <span class="date" class:abs={settings.dateFormat === 'absolute'}>{dateText(e.time)}</span>
                {#if e.oldPath}
                  <span class="ren" title="{e.oldPath} → {e.path}"><ArrowRightLeft size={11} strokeWidth={1.75} /></span>
                {/if}
                <span class="subj">{e.subject}</span>
                <span class="author">{e.author}</span>
              </div>
            {/if}
          {/snippet}
        </VirtualList>
      {:else if error}
        <div class="ph empty"><span class="msg selectable">{error}</span></div>
      {:else if !loading}
        <div class="ph empty"><RotateCcwClock size={20} strokeWidth={1.5} /></div>
      {/if}
    </div>
    <div class="main">
      {#if entry && target && fileSpec}
        <div class="pane">
          {#if mode === 'diff'}
            <DiffContent
              bind:this={content}
              {target}
              path={entry.path}
              oldPath={entry.oldPath}
              status={entry.status}
            />
          {:else}
            <FileViewer bind:this={viewer} spec={fileSpec} path={entry.path} />
          {/if}
        </div>
      {/if}
    </div>
  </div>
</div>

<style>
  .hv {
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
  .arrow {
    margin: 0 4px;
    color: var(--text-2);
  }
  .count {
    flex: none;
    font-family: var(--font-mono);
    font-size: var(--fs-xs);
    color: var(--text-2);
    font-variant-numeric: tabular-nums;
  }
  .body {
    position: relative;
    flex: 1;
    min-height: 0;
    display: flex;
  }
  .list {
    position: relative;
    flex: none;
    width: 320px;
    border-right: 1px solid var(--border);
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
    gap: var(--sp-2);
    height: 100%;
    padding: 0 10px;
    font-size: var(--fs-sm);
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
  .date {
    flex: none;
    width: 28px;
    color: var(--text-2);
    font-size: var(--fs-xs);
    font-variant-numeric: tabular-nums;
  }
  .date.abs {
    width: 66px;
  }
  .ren {
    display: inline-flex;
    flex: none;
    color: var(--text-2);
  }
  .subj {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
    color: var(--text-0);
  }
  .author {
    flex: 0 1 auto;
    max-width: 96px;
    min-width: 0;
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
    color: var(--text-1);
    font-size: var(--fs-xs);
  }
  .ph {
    position: absolute;
    inset: 0;
    display: flex;
    align-items: center;
    justify-content: center;
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
