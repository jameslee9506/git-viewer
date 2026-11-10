<!--
  Ref labels of one graph row. A local branch and its remote counterparts on the same row
  (`x` ↔ `origin/x`) merge into one badge with a cloud mark; a local branch checked out in another
  worktree gets a folder mark, as a detached worktree's label does. Shows the first `max` badges + "+N".
  `column`: the branch / tag column's bigger badges (the parent bounds their width), the current
  branch's check before the name and the kind's icon after it; hovering a collapsed group there expands
  it: RefPopover (one for the app) lists every ref of the row over the group.
-->
<script module lang="ts">
  import Archive from '@lucide/svelte/icons/archive';
  import Cloud from '@lucide/svelte/icons/cloud';
  import Folder from '@lucide/svelte/icons/folder';
  import Laptop from '@lucide/svelte/icons/laptop';
  import Tag from '@lucide/svelte/icons/tag';
  import type { RefLabel, RefLabelKind } from '../../types';

  const NO_WORKTREES: ReadonlyMap<string, string> = new Map();

  /** A collapsed group expands this long after the pointer enters it (a pointer crossing the rows must not pop them). */
  const EXPAND_MS = 120;

  /**
   * Column badges: the icon after the name, by kind (none for a detached HEAD). A local branch checked
   * out in another worktree gets a folder instead; merged remotes add a cloud.
   */
  export const KIND_ICONS: Partial<Record<RefLabelKind, typeof Cloud>> = {
    local: Laptop,
    remote: Cloud,
    tag: Tag,
    stash: Archive,
    worktree: Folder,
  };

  /** One label of a row: a ref, or a local branch with its remote counterparts merged in. */
  export interface Badge {
    kind: RefLabelKind;
    name: string;
    current: boolean;
    /** A local branch checked out in another worktree. */
    wt: boolean;
    /** Remote branches merged into this local badge (`origin/x`). */
    remotes: string[];
    /** The name, with the worktree path and the merged remotes on further lines. */
    title: string;
  }

  /** Path of the worktree a label stands for (detached HEAD) or its local branch is checked out in. */
  function pathOf(r: RefLabel, worktrees: ReadonlyMap<string, string>): string | undefined {
    if (r.kind === 'worktree') return r.fullName;
    return r.kind === 'local' ? worktrees.get(r.fullName) : undefined;
  }

  export function toBadges(labels: RefLabel[], worktrees: ReadonlyMap<string, string>): Badge[] {
    const all: Badge[] = labels.map((r) => {
      const path = pathOf(r, worktrees);
      return {
        kind: r.kind,
        name: r.name,
        current: r.current,
        wt: r.kind === 'local' && !!path,
        remotes: [],
        title: path ? `${r.name}\n${path}` : r.name,
      };
    });
    const locals = new Map<string, Badge>();
    for (const b of all) if (b.kind === 'local' && !locals.has(b.name)) locals.set(b.name, b);
    if (locals.size === 0) return all;
    return all.filter((b) => {
      if (b.kind !== 'remote') return true;
      const slash = b.name.indexOf('/');
      const local = slash > 0 ? locals.get(b.name.slice(slash + 1)) : undefined;
      if (!local) return true;
      local.remotes.push(b.name);
      local.title += `\n${b.name}`;
      return false;
    });
  }
</script>

<script lang="ts">
  import { onDestroy, untrack } from 'svelte';
  import Check from '@lucide/svelte/icons/check';
  import { closeRefPopoverSoon, hideRefPopover, refPopover, showRefPopover } from './RefPopover.svelte';

  interface Props {
    refs: RefLabel[];
    max?: number;
    /** Local branches checked out in another worktree: full ref name → worktree path. */
    worktrees?: ReadonlyMap<string, string>;
    /** Badges of the branch / tag column instead of the inline ones before the subject. */
    column?: boolean;
  }

  let { refs, max = 2, worktrees = NO_WORKTREES, column = false }: Props = $props();

  const badges = $derived(toBadges(refs, worktrees));
  const shown = $derived(badges.length > max ? badges.slice(0, max) : badges);
  const more = $derived(badges.length - shown.length);
  /** Inline "+N": every ref of the row, with the path of those tied to a worktree (the column's expands instead). */
  const moreTitle = $derived(
    more > 0 && !column
      ? refs
          .map((r) => {
            const path = pathOf(r, worktrees);
            return path ? `${r.name} · ${path}` : r.name;
          })
          .join('\n')
      : undefined,
  );

  // ───────────── hover expansion (column) ─────────────

  let group: HTMLSpanElement | undefined = $state();
  let expandTimer: ReturnType<typeof setTimeout> | undefined;
  /** This instance's identity as the popover's owner (another row's hover takes it over). */
  const me = {};

  function onEnter(): void {
    if (!column || more === 0) return;
    clearTimeout(expandTimer);
    expandTimer = setTimeout(() => {
      if (group) showRefPopover(me, group.getBoundingClientRect(), badges);
    }, EXPAND_MS);
  }

  function onLeave(): void {
    clearTimeout(expandTimer);
    // The popover covers the group: the pointer leaving it either entered the popover or left both.
    if (refPopover.owner === me) closeRefPopoverSoon();
  }

  // The refs changed under the open popover (a reload, or this row instance shows another row).
  $effect(() => {
    void badges;
    untrack(() => {
      if (refPopover.owner === me) hideRefPopover();
    });
  });

  onDestroy(() => {
    clearTimeout(expandTimer);
    if (refPopover.owner === me) hideRefPopover();
  });
</script>

<span
  class="refs"
  class:col={column}
  role="presentation"
  bind:this={group}
  onpointerenter={onEnter}
  onpointerleave={onLeave}
>
  {#each shown as b}
    {@const Kind = column ? (b.wt ? Folder : KIND_ICONS[b.kind]) : undefined}
    <span class="badge badge-{b.kind}" class:current={b.current} class:tail={!!Kind} title={b.title}>
      {#if column}
        {#if b.current}
          <Check size={12} strokeWidth={2} />
        {/if}
        <span class="nm">{b.name}</span>
        {#if Kind}
          <Kind class="kind" strokeWidth={1.75} />
        {/if}
        {#if b.remotes.length}
          <Cloud class="kind" strokeWidth={1.75} />
        {/if}
      {:else}
        {#if b.current}
          <Check size={10} strokeWidth={2.5} />
        {:else if b.kind === 'tag'}
          <Tag size={10} strokeWidth={2} />
        {:else if b.kind === 'worktree' || b.wt}
          <Folder size={10} strokeWidth={2} />
        {/if}
        <span class="nm">{b.name}</span>
        {#if b.remotes.length}
          <Cloud size={10} strokeWidth={2} />
        {/if}
      {/if}
    </span>
  {/each}
  {#if more > 0}
    <span class="badge more" title={moreTitle}>+{more}</span>
  {/if}
</span>

<style>
  .refs {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    flex: 0 1 auto;
    min-width: 0;
    max-width: 70%;
    overflow: hidden;
  }
  .refs .badge {
    flex: 0 1 auto;
    min-width: 0;
  }
  .refs .badge :global(svg) {
    flex: none;
  }
  .nm {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    min-width: 0;
  }
  .refs .more {
    flex: none;
    background: var(--badge-more-bg);
    color: var(--badge-more-fg);
  }

  /* Branch / tag column: the parent bounds the width; the names ellipsize, the icons never shrink.
     Neutral near-square labels whatever the kind (the kind shows in the icon; the hue stays in the
     lanes); the current branch has a blue-tinted fill and primary text at the same weight. */
  .refs.col {
    max-width: none;
  }
  .refs.col .badge {
    gap: 4px;
    height: var(--ref-label-h);
    padding: 0 6px;
    border-radius: var(--radius-xs);
    font-size: var(--fs-sm);
    line-height: var(--ref-label-h);
    background: var(--ref-label-bg);
    color: var(--ref-label-fg);
    font-weight: 400;
  }
  .refs.col .badge.current {
    background: var(--ref-label-current-bg);
    color: var(--text-0);
  }
  /* The kind icons after the name are a step bigger than the check. A label ending in them keeps the
     icons centered in its end (as far from the right edge as from the top and bottom) instead of
     holding them to the text padding, so a label with one icon is no wider for it. */
  .refs.col .badge.tail {
    padding-right: var(--ref-icon-inset);
  }
  .refs.col .badge :global(svg.kind) {
    width: var(--ref-icon-size);
    height: var(--ref-icon-size);
  }
  /* Equal shares capped at the natural width: a short tag beside a long branch stays whole and only
     the long name ellipsizes (shrinking by natural width would squeeze the short one to its padding).
     The container's own width is still the sum of the natural widths. */
  .refs.col .badge:not(.more) {
    flex: 1 1 0;
    max-width: max-content;
  }
  /* The "+N" chip is one more label (the hover expansion stands in for its tooltip). */
  .refs.col .more {
    background: var(--ref-label-bg);
    color: var(--ref-label-fg);
    font-variant-numeric: tabular-nums;
  }
</style>
