<!--
  Diff viewer (repo.view kind 'diff'): bar = back, status, path, +a −d · file stepping, unified/split,
  whitespace, context (hunks ↔ full file), blame, history; below it DiffContent for the current item.
  A narrow bar drops +a −d (then i/n) instead of clipping its buttons.
  Keys (active tab only): ↑ ↓ and [ ] file, ← back to the history list, Esc back one viewer, n p /
  ⌥↓ ⌥↑ change block, s split, w whitespace. The arrows belong to the file list the diff is one of
  (a level below the graph row whose → opened it), so they scroll no diff: its list keeps PgUp PgDn
  Space Home End.
-->
<script lang="ts">
  import ArrowLeft from '@lucide/svelte/icons/arrow-left';
  import ChevronDown from '@lucide/svelte/icons/chevron-down';
  import ChevronUp from '@lucide/svelte/icons/chevron-up';
  import Columns2 from '@lucide/svelte/icons/columns-2';
  import FoldVertical from '@lucide/svelte/icons/fold-vertical';
  import RotateCcwClock from '@lucide/svelte/icons/rotate-ccw-clock';
  import Rows2 from '@lucide/svelte/icons/rows-2';
  import ScanText from '@lucide/svelte/icons/scan-text';
  import UnfoldVertical from '@lucide/svelte/icons/unfold-vertical';
  import WholeWord from '@lucide/svelte/icons/whole-word';
  import { untrack } from 'svelte';
  import { app } from '../../state/app.svelte';
  import { diffItemKey, getRepo, type CenterView, type DiffItem } from '../../state/repo.svelte';
  import { settings } from '../../state/settings.svelte';
  import { contextMenu } from '../../state/ui.svelte';
  import type { FileDiff } from '../../types';
  import { basename, dirname } from '../../util/format';
  import { isComposing, isEditable } from '../../util/keys';
  import { historyFor } from '../details/filetree';
  import DiffContent, { diffPrefs, setFullFile } from './DiffContent.svelte';
  import { blockNavKey, submoduleShas } from './model';

  const ICON = 14;
  const STROKE = 1.75;

  const repo = getRepo();

  const view = $derived(repo.view.kind === 'diff' ? repo.view : null);
  const item = $derived(view ? (view.items[view.index] ?? null) : null);
  const key = $derived(item ? diffItemKey(item) : '');
  let content: DiffContent | undefined = $state();
  /** Last diff DiffContent showed, tagged with the item it belongs to. */
  let loaded = $state.raw<{ key: string; diff: FileDiff | null } | null>(null);
  const diff = $derived(loaded && loaded.key === key ? loaded.diff : null);
  const blame = $derived(item ? blameView(item, diff) : null);
  // Same start as the file list's File History: the diffed commit / range side / stash base, or the
  // HEAD of the worktree holding the change (null for files without history).
  const history = $derived(item ? historyFor(item, repo.id, repo.refs) : null);
  const renamed = $derived(!!item && item.file.oldPath !== null && item.file.oldPath !== item.file.path);

  // Take keyboard focus when nothing else holds it (e.g. opened from the graph, which is now inert).
  $effect(() => {
    const c = content;
    const a = document.activeElement;
    if (c && (!a || a === document.body || a.closest('[inert]') !== null)) untrack(() => c.focus());
  });

  /**
   * Blame of the file at the side shown by the diff (deleted files: the old side). The same rule as the
   * file lists' Blame (`blameView` in details/filetree.ts): untracked files, a stash's included, have
   * nothing to blame (its `^3` commit is a parentless snapshot that would claim every line).
   */
  function blameView(it: DiffItem, d: FileDiff | null): CenterView | null {
    const t = it.target;
    const f = it.file;
    if (d && (d.binary || submoduleShas(d) !== null)) return null;
    const del = f.status === 'D';
    switch (t.kind) {
      case 'commit':
        return { kind: 'blame', path: f.path, rev: del ? (t.parent ?? `${t.oid}^`) : t.oid, cwd: null };
      case 'range':
        return { kind: 'blame', path: f.path, rev: del ? t.from : t.to, cwd: null };
      case 'stash':
        return { kind: 'blame', path: f.path, rev: del ? `${t.oid}^1` : t.oid, cwd: null };
      case 'staged':
      case 'unstaged':
      case 'conflicted':
        return { kind: 'blame', path: f.path, rev: del ? 'HEAD' : null, cwd: t.cwd };
      case 'worktree':
        return { kind: 'blame', path: f.path, rev: del ? t.oid : null, cwd: t.cwd };
      case 'stashUntracked':
      case 'untracked':
        return null;
    }
  }

  function toggleSplit() {
    settings.diffMode = settings.diffMode === 'split' ? 'unified' : 'split';
  }

  /**
   * ←: back one viewer level as Esc, and where that is the graph its list takes the keys — also from
   * a file list, where Esc leaves the focus: ← leads out of the files, to the history.
   */
  function leave() {
    repo.back();
    if (repo.view.kind === 'graph') repo.graphFocusRequest++;
  }

  function onkeydown(e: KeyboardEvent) {
    if (!view || e.defaultPrevented || isComposing(e)) return;
    if (app.active?.repo !== repo || contextMenu.open || isEditable(e.target)) return;
    if (e.metaKey || e.ctrlKey) return;
    const dir = blockNavKey(e);
    if (dir) {
      e.preventDefault();
      content?.step(dir);
      return;
    }
    if (e.altKey || e.shiftKey) return;
    switch (e.code) {
      case 'Escape':
        // One viewer level per press: a held Esc must not unwind the whole stack.
        if (!e.repeat) repo.back();
        break;
      // The arrows reach here unless a focused list took them (a file list moves its own cursor, and
      // the diff follows): ↑ ↓ walk the files, ← leaves them. → is taken too, so that no arrow moves
      // a diff sideways that ← would not bring back.
      case 'ArrowLeft':
        if (!e.repeat) leave();
        break;
      case 'ArrowRight':
        break;
      case 'ArrowUp':
      case 'BracketLeft':
        repo.stepDiff(-1);
        break;
      case 'ArrowDown':
      case 'BracketRight':
        repo.stepDiff(1);
        break;
      // Toggles ignore key auto-repeat (holding the key would flip them back and forth).
      case 'KeyS':
        if (!e.repeat) toggleSplit();
        break;
      case 'KeyW':
        if (!e.repeat) settings.ignoreWhitespace = !settings.ignoreWhitespace;
        break;
      default:
        return;
    }
    e.preventDefault();
  }
</script>

<svelte:window {onkeydown} />

{#snippet pathText(p: string)}
  {@const dir = dirname(p)}
  {#if dir}<span class="dir">{dir}/</span>{/if}<span class="name">{basename(p)}</span>
{/snippet}

<div class="dv">
  {#if view && item}
    {@const f = item.file}
    <div class="panel-header bar" class:multi={view.items.length > 1}>
      <button class="icon-btn" title="Back (← or Esc)" onclick={() => repo.back()}>
        <ArrowLeft size={ICON} strokeWidth={STROKE} />
      </button>
      <span class="st st-{f.status === '?' ? 'q' : f.status}">{f.status}</span>
      <span class="path ellipsis selectable" title={renamed ? `${f.oldPath} → ${f.path}` : f.path}>
        {#if renamed && f.oldPath}{@render pathText(f.oldPath)}<span class="to">→</span>{/if}{@render pathText(f.path)}
      </span>
      {#if diff && (diff.additions > 0 || diff.deletions > 0)}
        <span class="counts"><span class="add">+{diff.additions}</span> <span class="del">−{diff.deletions}</span></span>
      {/if}
      <span class="grow"></span>
      {#if view.items.length > 1}
        <button
          class="icon-btn"
          title="Previous file (↑ or [)"
          disabled={view.index <= 0}
          onclick={() => repo.stepDiff(-1)}
        >
          <ChevronUp size={ICON} strokeWidth={STROKE} />
        </button>
        <button
          class="icon-btn"
          title="Next file (↓ or ])"
          disabled={view.index >= view.items.length - 1}
          onclick={() => repo.stepDiff(1)}
        >
          <ChevronDown size={ICON} strokeWidth={STROKE} />
        </button>
        <span class="pos">{view.index + 1}/{view.items.length}</span>
      {/if}
      <div class="seg">
        <button
          class:on={settings.diffMode !== 'split'}
          title="Unified (s)"
          onclick={() => (settings.diffMode = 'unified')}
        >
          <Rows2 size={ICON} strokeWidth={STROKE} />
        </button>
        <button class:on={settings.diffMode === 'split'} title="Split (s)" onclick={() => (settings.diffMode = 'split')}>
          <Columns2 size={ICON} strokeWidth={STROKE} />
        </button>
      </div>
      <button
        class="icon-btn"
        class:on={settings.ignoreWhitespace}
        title="Ignore whitespace (w)"
        onclick={() => (settings.ignoreWhitespace = !settings.ignoreWhitespace)}
      >
        <WholeWord size={ICON} strokeWidth={STROKE} />
      </button>
      <button
        class="icon-btn"
        class:on={diffPrefs.fullFile}
        title={diffPrefs.fullFile ? 'Hunks' : 'Full file'}
        onclick={() => setFullFile(!diffPrefs.fullFile)}
      >
        {#if diffPrefs.fullFile}
          <FoldVertical size={ICON} strokeWidth={STROKE} />
        {:else}
          <UnfoldVertical size={ICON} strokeWidth={STROKE} />
        {/if}
      </button>
      <button class="icon-btn" title="Blame" disabled={!blame} onclick={() => blame && repo.openView(blame)}>
        <ScanText size={ICON} strokeWidth={STROKE} />
      </button>
      <button class="icon-btn" title="History" disabled={!history} onclick={() => history && repo.openView(history)}>
        <RotateCcwClock size={ICON} strokeWidth={STROKE} />
      </button>
    </div>
    <div class="body">
      <DiffContent
        bind:this={content}
        target={item.target}
        path={f.path}
        oldPath={f.oldPath}
        status={f.status}
        sideKeys={false}
        onloaded={(d) => (loaded = { key, diff: d })}
      />
    </div>
  {/if}
</div>

<style>
  .dv {
    display: flex;
    flex-direction: column;
    height: 100%;
    min-width: 0;
    background: var(--bg-1);
    /* Its width comes from the viewer layer, so it can be the bar's query container. */
    container-type: inline-size;
  }
  .bar {
    height: var(--viewer-bar-h);
    gap: 6px;
    padding: 0 6px;
  }
  .path {
    flex: 0 1 auto;
    min-width: 0;
    font-size: var(--fs-sm);
    color: var(--text-0);
  }
  .dir {
    color: var(--text-2);
  }
  .to {
    margin: 0 6px;
    color: var(--text-2);
  }
  .counts,
  .pos {
    flex: none;
    font-family: var(--font-mono);
    font-size: var(--fs-xs);
    white-space: nowrap;
  }
  .pos {
    min-width: 32px;
    margin-right: 4px;
    color: var(--text-2);
  }
  .seg {
    flex: none;
    margin: 0 2px;
  }
  .body {
    position: relative;
    flex: 1 1 auto;
    min-height: 0;
  }
  /* The center can be 420px (less when zoomed). Besides path and counts the bar takes 364px with file
     stepping, 262px without: +a −d, then i/n give way so no button is clipped. */
  @container (max-width: 470px) {
    .multi .counts {
      display: none;
    }
  }
  @container (max-width: 360px) {
    .counts,
    .pos {
      display: none;
    }
    .bar {
      gap: 4px;
    }
  }
</style>
