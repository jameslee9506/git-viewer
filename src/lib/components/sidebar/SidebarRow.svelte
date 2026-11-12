<!--
  One sidebar row: section header (icon, name, count; 4px taller than the rest) or a folder / leaf (branch,
  remote branch, tag, stash, worktree, submodule) behind its type icon, filter matches highlighted.
  The checked-out branch (a check box in the chevron's slot) and the current worktree lie on a green band.
  In the simple view the branch it follows is marked (a bolder name, a target at the right end) and a
  branch kept only on a remote gets a cloud after its name.
  Purely prop-driven: VirtualList recycles instances by index.
-->
<script module lang="ts">
  import Archive from '@lucide/svelte/icons/archive';
  import Box from '@lucide/svelte/icons/box';
  import Boxes from '@lucide/svelte/icons/boxes';
  import Cloud from '@lucide/svelte/icons/cloud';
  import FolderGit2 from '@lucide/svelte/icons/folder-git-2';
  import GitBranch from '@lucide/svelte/icons/git-branch';
  import House from '@lucide/svelte/icons/house';
  import Laptop from '@lucide/svelte/icons/laptop';
  import Tag from '@lucide/svelte/icons/tag';
  import type { Leaf, SectionId } from './tree';

  type IconType = typeof GitBranch;

  const SECTION_ICONS: Record<SectionId, IconType> = {
    local: Laptop,
    remote: Cloud,
    worktrees: FolderGit2,
    stashes: Archive,
    submodules: Boxes,
    tags: Tag,
    branches: GitBranch,
  };

  const LEAF_ICONS: Record<Leaf['kind'], IconType> = {
    local: GitBranch,
    remote: GitBranch,
    tag: Tag,
    stash: Archive,
    worktree: FolderGit2,
    submodule: Box,
  };

  /** Type icon of a leaf; the current worktree gets a house (the checked-out branch is marked by its check box). */
  function leafIcon(n: Leaf): IconType {
    if (n.kind === 'worktree' && n.ref.isCurrent) return House;
    return LEAF_ICONS[n.kind];
  }

  /**
   * Left padding of a row and the indent per folder level, px. Rows directly in a section line up with its
   * header (lead slot under its chevron, type icon under its icon); a child inside a folder has its icon
   * under its parent's name.
   */
  const PAD = 8;
  const INDENT = 16;
</script>

<script lang="ts">
  import Check from '@lucide/svelte/icons/check';
  import ChevronRight from '@lucide/svelte/icons/chevron-right';
  import CloudOff from '@lucide/svelte/icons/cloud-off';
  import Eye from '@lucide/svelte/icons/eye';
  import EyeOff from '@lucide/svelte/icons/eye-off';
  import FilePen from '@lucide/svelte/icons/file-pen';
  import Folder from '@lucide/svelte/icons/folder';
  import FolderOpen from '@lucide/svelte/icons/folder-open';
  import FolderX from '@lucide/svelte/icons/folder-x';
  import GitCommitHorizontal from '@lucide/svelte/icons/git-commit-horizontal';
  import Globe from '@lucide/svelte/icons/globe';
  import Lock from '@lucide/svelte/icons/lock';
  import Target from '@lucide/svelte/icons/target';
  import TriangleExclamationPoint from '@lucide/svelte/icons/triangle-exclamation-point';
  import { absoluteTime, relativeTime, shortOid } from '../../util/format';
  import { SECTION_TITLES, expandable, matchPieces, trackLabel, worktreeDetail, type Row } from './tree';

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
    /** Branch leaf (simple view): the branch the simple view follows. */
    focused: boolean;
    /** Remote branch leaf (simple view): listed among the branches, as no local branch has its name. */
    remoteOnly: boolean;
    /** Lower-cased filter terms ([] = no filter): their matches in the labels are highlighted. */
    terms: string[];
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
    focused,
    remoteOnly,
    terms,
    onclick,
    ondblclick,
    oncontextmenu,
    onhide,
    onsolo,
  }: Props = $props();

  const canExpand = $derived(row.type !== 'leaf' && expandable(row));

  /** The checked-out branch: a check box in the lead slot. */
  const checkedOut = $derived(row.type === 'leaf' && row.node.kind === 'local' && row.node.ref.isHead);

  /** The checked-out branch or the current worktree: a green band. */
  const current = $derived(
    checkedOut || (row.type === 'leaf' && row.node.kind === 'worktree' && row.node.ref.isCurrent),
  );

  /** Name drawn faint: not in the graph, uninitialized submodule or prunable worktree. */
  const off = $derived.by(() => {
    if (hidden || dimmed) return true;
    if (row.type !== 'leaf') return false;
    const n = row.node;
    return (n.kind === 'submodule' && n.ref.state === 'uninitialized') || (n.kind === 'worktree' && n.ref.prunable);
  });

  const tip = $derived(tooltip(row));

  function tooltip(r: Row): string | undefined {
    // A header shows its name already.
    if (r.type === 'section') return undefined;
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

<!-- A label with its filter matches marked (one text node when there is no filter). -->
{#snippet marked(text: string)}
  {#if terms.length === 0}
    {text}
  {:else}
    {#each matchPieces(text, terms) as p}
      {#if p.hit}<mark class="hl">{p.text}</mark>{:else}{p.text}{/if}
    {/each}
  {/if}
{/snippet}

<!-- Keyboard input is handled by the list container (roving cursor), not per row. -->
<!-- svelte-ignore a11y_click_events_have_key_events, a11y_interactive_supports_focus -->
<div
  class="row"
  class:section={row.type === 'section'}
  class:current
  class:cursor
  class:sep={separated}
  role="treeitem"
  aria-selected={cursor}
  aria-level={row.depth + 1}
  aria-expanded={canExpand && row.type !== 'leaf' ? row.expanded : undefined}
  title={tip}
  style:padding-left="{PAD + Math.max(0, row.depth - 1) * INDENT}px"
  {onclick}
  {ondblclick}
  {oncontextmenu}
>
  <span class="lead">
    {#if canExpand && row.type !== 'leaf'}
      <span class="chev" class:open={row.expanded}><ChevronRight size={12} strokeWidth={2} /></span>
    {:else if checkedOut}
      <span class="check"><Check size={11} strokeWidth={3} /></span>
    {/if}
  </span>

  {#if row.type === 'section'}
    {@const Icon = SECTION_ICONS[row.section.id]}
    <span class="ticon"><Icon size={14} strokeWidth={1.75} /></span>
    <span class="label ellipsis">{SECTION_TITLES[row.section.id]}</span>
    <span class="total">{row.section.count}</span>
  {:else if row.type === 'folder'}
    {@const Icon = row.node.remote ? Globe : canExpand && row.expanded ? FolderOpen : Folder}
    <span class="ticon" class:off><Icon size={14} strokeWidth={1.75} /></span>
    <span class="name folder ellipsis" class:off>{@render marked(row.node.label)}</span>
    {#if canExpand && !row.expanded}<span class="count">{row.node.count}</span>{/if}
    <span class="fill"></span>
  {:else}
    {@const n = row.node}
    {@const Icon = leafIcon(n)}
    <span class="ticon" class:cur={current} class:off><Icon size={14} strokeWidth={current ? 2 : 1.75} /></span>
    <span class="name ellipsis" class:current class:focused class:off>{@render marked(n.label)}</span>
    {#if remoteOnly && n.kind === 'remote'}
      <span class="mark" title={n.ref.name}><Cloud size={12} strokeWidth={1.75} /></span>
    {/if}
    {#if n.kind === 'worktree'}
      {@const detail = worktreeDetail(n.ref)}
      {#if detail && detail !== n.label}
        <span class="wt">
          <span class="dicon">
            {#if n.ref.branch}
              <GitBranch size={12} strokeWidth={1.75} />
            {:else}
              <GitCommitHorizontal size={12} strokeWidth={1.75} />
            {/if}
          </span>
          <span class="detail ellipsis" class:mono={!n.ref.branch}>{@render marked(detail)}</span>
        </span>
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
    {#if focused}
      <span class="followed" title="Followed"><Target size={12} strokeWidth={1.75} /></span>
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
    padding-right: 8px;
    font-size: var(--fs);
    color: var(--text-0);
    white-space: nowrap;
  }
  /* Section headers read as dividers: a rule above all but the first row, 2px above and below the row
     content (see the sidebar's SECTION_EXTRA). Their background is the panel's, painted all the same:
     the pinned copy covers the rows scrolling under it. */
  .row.section {
    padding-top: 2px;
    padding-bottom: 2px;
    background-color: var(--bg-2);
  }
  .row.sep {
    border-top: 1px solid var(--border);
  }
  /* The current rows' tint is an image over the row's background color: hover and the cursor show
     through it instead of replacing it. */
  .row.current {
    background-image: linear-gradient(var(--bg-current), var(--bg-current));
  }
  /* Hover and cursor win over the header's background. Colors only: the shorthand would drop the tint. */
  .row:hover {
    background-color: var(--bg-hover);
  }
  .row.cursor {
    background-color: var(--bg-active);
  }
  :global(.sb-tree:focus-within) .row.cursor {
    background-color: var(--bg-active-strong);
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
  /* The checked-out branch's check box: the amend box's geometry, in green. */
  .check {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 14px;
    height: 14px;
    border-radius: var(--radius-xs);
    background: var(--current);
    color: var(--on-current);
  }

  .ticon {
    flex: none;
    display: inline-flex;
    color: var(--text-2);
  }
  .ticon.cur {
    color: var(--accent);
  }
  .ticon.off {
    color: var(--text-3);
  }

  .label {
    flex: 0 1 auto;
    font-size: var(--fs-label);
    font-weight: 700;
    letter-spacing: 0.06em;
    text-transform: uppercase;
    color: var(--text-1);
  }
  .total {
    flex: none;
    margin-left: auto;
    padding-left: 6px;
    font-family: var(--font-mono);
    font-size: var(--fs-xs);
    color: var(--accent);
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
  .name.folder {
    color: var(--text-1);
  }
  .name.current {
    color: var(--accent);
    font-weight: 500;
  }
  /* The branch the simple view follows (after .current: a checked-out branch it follows takes this weight). */
  .name.focused {
    color: var(--text-0);
    font-weight: 600;
  }
  .name.off {
    color: var(--text-2);
  }
  .hl {
    border-radius: 2px;
    background: var(--hl-bg);
    color: var(--hl-fg);
  }
  /* Matches stay marked on a faint name, without lighting it up. */
  .name.off .hl {
    background: color-mix(in srgb, var(--hl-bg) 50%, transparent);
    color: inherit;
  }
  /* A worktree's branch (or commit): its icon shrinks away with the text, never left on its own. */
  .wt {
    flex: 0 10 auto;
    display: inline-flex;
    align-items: center;
    gap: 2px;
    min-width: 0;
    margin-left: 2px;
    overflow: hidden;
    color: var(--text-2);
  }
  .dicon {
    flex: none;
    display: inline-flex;
  }
  .detail {
    flex: 0 1 auto;
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
  /* The simple view's mark on the branch it follows: a target in primary ink, not a button. */
  .followed {
    flex: none;
    display: inline-flex;
    color: var(--text-0);
  }
  /* Muted ink a tier lighter on the green band: --text-2 is too faint there. */
  .row.current .wt,
  .row.current .meta,
  .row.current .mark,
  .row.current .name.off {
    color: var(--text-1);
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
