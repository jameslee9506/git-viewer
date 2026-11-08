<!--
  One fixed-height sidebar row: section header, folder or leaf (branch, remote branch, tag, stash,
  worktree, submodule). Purely prop-driven: VirtualList recycles instances by index.
-->
<script module lang="ts">
  import Archive from '@lucide/svelte/icons/archive';
  import Boxes from '@lucide/svelte/icons/boxes';
  import Cloud from '@lucide/svelte/icons/cloud';
  import FolderGit2 from '@lucide/svelte/icons/folder-git-2';
  import GitBranch from '@lucide/svelte/icons/git-branch';
  import Tag from '@lucide/svelte/icons/tag';
  import type { SectionId } from './tree';

  const SECTION_ICONS: Record<SectionId, typeof GitBranch> = {
    worktrees: FolderGit2,
    local: GitBranch,
    remote: Cloud,
    tags: Tag,
    stashes: Archive,
    submodules: Boxes,
  };

  /** Left padding of a depth-0 row and the indent per level, px. */
  const PAD = 6;
  const INDENT = 12;
</script>

<script lang="ts">
  import ChevronRight from '@lucide/svelte/icons/chevron-right';
  import CloudOff from '@lucide/svelte/icons/cloud-off';
  import Eye from '@lucide/svelte/icons/eye';
  import EyeOff from '@lucide/svelte/icons/eye-off';
  import FilePen from '@lucide/svelte/icons/file-pen';
  import Folder from '@lucide/svelte/icons/folder';
  import FolderX from '@lucide/svelte/icons/folder-x';
  import Lock from '@lucide/svelte/icons/lock';
  import Target from '@lucide/svelte/icons/target';
  import TriangleExclamationPoint from '@lucide/svelte/icons/triangle-exclamation-point';
  import { absoluteTime, relativeTime, shortOid } from '../../util/format';
  import { SECTION_TITLES, expandable, trackLabel, worktreeDetail, type Row } from './tree';

  interface Props {
    row: Row;
    /** Keyboard cursor (also the last clicked row). */
    cursor: boolean;
    /** Draw a separator above (section headers after the first row). */
    separated: boolean;
    /** Leaf: hidden from the graph. Group: every ref below is hidden. */
    hidden: boolean;
    /** Leaf: this ref is the solo ref. */
    soloed: boolean;
    /** Leaf: left out of the graph because another ref is soloed. */
    dimmed: boolean;
    /** Show the hide (eye) toggle; leaves also get the solo toggle. */
    hideable: boolean;
    /** Worktree leaf: number of changed paths (null = unknown). */
    changes: number | null;
    onclick: (e: MouseEvent) => void;
    ondblclick: (e: MouseEvent) => void;
    oncontextmenu: (e: MouseEvent) => void;
    onhide: () => void;
    onsolo: () => void;
  }

  let {
    row,
    cursor,
    separated,
    hidden,
    soloed,
    dimmed,
    hideable,
    changes,
    onclick,
    ondblclick,
    oncontextmenu,
    onhide,
    onsolo,
  }: Props = $props();

  const canExpand = $derived(row.type !== 'leaf' && expandable(row));

  const current = $derived.by(() => {
    if (row.type !== 'leaf') return false;
    const n = row.node;
    return (n.kind === 'local' && n.ref.isHead) || (n.kind === 'worktree' && n.ref.isCurrent);
  });

  /** Name drawn faint: not in the graph, uninitialized submodule or prunable worktree. */
  const off = $derived.by(() => {
    if (hidden || dimmed) return true;
    if (row.type !== 'leaf') return false;
    const n = row.node;
    return (n.kind === 'submodule' && n.ref.state === 'uninitialized') || (n.kind === 'worktree' && n.ref.prunable);
  });

  const tip = $derived(tooltip(row));

  function tooltip(r: Row): string {
    if (r.type === 'section') return SECTION_TITLES[r.section.id];
    if (r.type === 'folder') {
      const url = r.node.remote?.url;
      return url ? `${r.node.label}\n${url}` : r.node.label;
    }
    const n = r.node;
    switch (n.kind) {
      case 'local': {
        const b = n.ref;
        let t = b.name;
        if (b.upstream) t += ` → ${b.upstream}${b.upstreamGone ? ' (gone)' : ''}`;
        if (b.worktree && !b.isHead) t += `\n${b.worktree}`;
        return t;
      }
      case 'remote':
        return n.ref.name;
      case 'tag':
        return n.ref.message ? `${n.ref.name}\n${n.ref.message}` : n.ref.name;
      case 'stash':
        return `${n.ref.name} · ${absoluteTime(n.ref.time)}\n${n.ref.message}`;
      case 'worktree':
        return n.ref.path;
      case 'submodule':
        return `${n.ref.path}\n${n.ref.state} · ${shortOid(n.ref.oid)}`;
    }
  }

  /** Button click that stays off the row; ⌃-click is the macOS context click (the menu's). */
  function stop(action: () => void): (e: MouseEvent) => void {
    return (e) => {
      e.stopPropagation();
      if (!e.ctrlKey) action();
    };
  }

  function swallow(e: MouseEvent): void {
    e.stopPropagation();
  }
</script>

<!-- Keyboard input is handled by the list container (roving cursor), not per row. -->
<!-- svelte-ignore a11y_click_events_have_key_events, a11y_interactive_supports_focus -->
<div
  class="row"
  class:group={row.type !== 'leaf'}
  class:cursor
  class:sep={separated}
  role="treeitem"
  aria-selected={cursor}
  aria-level={row.depth + 1}
  aria-expanded={canExpand && row.type !== 'leaf' ? row.expanded : undefined}
  title={tip}
  style:padding-left="{PAD + row.depth * INDENT}px"
  {onclick}
  {ondblclick}
  {oncontextmenu}
>
  <span class="lead">
    {#if canExpand && row.type !== 'leaf'}
      <span class="chev" class:open={row.expanded}><ChevronRight size={12} strokeWidth={1.75} /></span>
    {:else if current}
      <span class="dot"></span>
    {/if}
  </span>

  {#if row.type === 'section'}
    {@const Icon = SECTION_ICONS[row.section.id]}
    <span class="sicon"><Icon size={14} strokeWidth={1.75} /></span>
    <span class="count">{row.section.count}</span>
    <span class="fill"></span>
  {:else if row.type === 'folder'}
    <span class="name folder ellipsis" class:off>{row.node.label}</span>
    {#if canExpand && !row.expanded}<span class="count">{row.node.count}</span>{/if}
    <span class="fill"></span>
  {:else}
    {@const n = row.node}
    <span class="name ellipsis" class:current class:off>{n.label}</span>
    {#if n.kind === 'worktree'}
      {@const detail = worktreeDetail(n.ref)}
      {#if detail && detail !== n.label}
        <span class="detail ellipsis" class:mono={!n.ref.branch}>{detail}</span>
      {/if}
    {/if}
    <span class="fill"></span>
    {#if n.kind === 'local'}
      {#if n.ref.upstreamGone}
        <span class="mark" title="{n.ref.upstream ?? ''} gone"><CloudOff size={12} strokeWidth={1.75} /></span>
      {:else if n.ref.ahead > 0 || n.ref.behind > 0}
        <span class="meta" title={n.ref.upstream ?? ''}>{trackLabel(n.ref.ahead, n.ref.behind)}</span>
      {/if}
      {#if n.ref.worktree && !n.ref.isHead}
        <span class="mark" title={n.ref.worktree}><Folder size={12} strokeWidth={1.75} /></span>
      {/if}
    {:else if n.kind === 'stash'}
      <span class="meta">{relativeTime(n.ref.time)}</span>
    {:else if n.kind === 'worktree'}
      {#if changes}
        <span class="meta changes"><FilePen size={11} strokeWidth={1.75} />{changes}</span>
      {/if}
      {#if n.ref.locked}
        <span class="mark" title="locked"><Lock size={12} strokeWidth={1.75} /></span>
      {/if}
      {#if n.ref.prunable}
        <span class="mark" title="prunable"><FolderX size={12} strokeWidth={1.75} /></span>
      {/if}
    {:else if n.kind === 'submodule'}
      {#if n.ref.state === 'conflict'}
        <span class="mark conflict" title="conflict"><TriangleExclamationPoint size={12} strokeWidth={1.75} /></span>
      {/if}
    {/if}
  {/if}

  {#if hideable}
    <button
      class="act"
      class:on={hidden}
      tabindex="-1"
      title={row.type === 'leaf' ? (hidden ? 'Show' : 'Hide') : hidden ? 'Show all' : 'Hide all'}
      onclick={stop(onhide)}
      ondblclick={swallow}
    >
      {#if hidden}<EyeOff size={12} strokeWidth={1.75} />{:else}<Eye size={12} strokeWidth={1.75} />{/if}
    </button>
    {#if row.type === 'leaf'}
      <button
        class="act solo"
        class:on={soloed}
        tabindex="-1"
        title={soloed ? 'Unsolo' : 'Solo'}
        onclick={stop(onsolo)}
        ondblclick={swallow}
      >
        <Target size={12} strokeWidth={1.75} />
      </button>
    {/if}
  {/if}
</div>

<style>
  .row {
    display: flex;
    align-items: center;
    gap: 4px;
    height: 100%;
    padding-right: 6px;
    font-size: var(--fs-sm);
    color: var(--text-0);
    white-space: nowrap;
  }
  .row:hover {
    background: var(--bg-hover);
  }
  .row.cursor {
    background: var(--bg-active);
  }
  :global(.sb-list:focus-within) .row.cursor {
    background: var(--bg-active-strong);
  }
  .row.sep {
    border-top: 1px solid var(--border);
  }
  .row.group {
    color: var(--text-1);
  }

  .lead {
    flex: none;
    width: 14px;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    color: var(--text-2);
  }
  .chev {
    display: inline-flex;
  }
  .chev.open {
    transform: rotate(90deg);
  }
  .dot {
    width: 5px;
    height: 5px;
    border-radius: 50%;
    background: var(--accent);
  }

  .sicon {
    display: inline-flex;
    flex: none;
  }
  .count {
    flex: none;
    font-family: var(--font-mono);
    font-size: var(--fs-xs);
    color: var(--text-2);
  }

  .name {
    flex: 0 1 auto;
  }
  .name.current {
    color: var(--accent);
    font-weight: 600;
  }
  .name.off {
    color: var(--text-3);
  }
  .detail {
    flex: 0 10 auto;
    color: var(--text-2);
  }
  .detail.mono {
    font-size: var(--fs-xs);
  }
  .fill {
    flex: 1 1 0;
  }

  .meta {
    flex: none;
    display: inline-flex;
    align-items: center;
    gap: 2px;
    font-family: var(--font-mono);
    font-size: var(--fs-xs);
    color: var(--text-2);
  }
  .mark {
    flex: none;
    display: inline-flex;
    color: var(--text-2);
  }
  .mark.conflict {
    color: var(--st-conflict);
  }

  .act {
    display: none;
    flex: none;
    width: 18px;
    height: 18px;
    align-items: center;
    justify-content: center;
    border-radius: var(--radius-sm);
    color: var(--text-2);
  }
  .row:hover .act,
  .act.on {
    display: inline-flex;
  }
  .act:hover {
    color: var(--text-0);
    background: var(--bg-active);
  }
  .act.solo.on {
    color: var(--accent);
  }
</style>
