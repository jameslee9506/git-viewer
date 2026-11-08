<!--
  Ref labels of one graph row. A local branch and its remote counterparts on the same row
  (`x` ↔ `origin/x`) merge into one badge with a cloud mark; a local branch checked out in another
  worktree gets a folder mark, as a detached worktree's label does. Shows the first `max` badges + "+N".
-->
<script module lang="ts">
  const NO_WORKTREES: ReadonlyMap<string, string> = new Map();
</script>

<script lang="ts">
  import Check from '@lucide/svelte/icons/check';
  import Cloud from '@lucide/svelte/icons/cloud';
  import Folder from '@lucide/svelte/icons/folder';
  import Tag from '@lucide/svelte/icons/tag';
  import type { RefLabel, RefLabelKind } from '../../types';

  interface Props {
    refs: RefLabel[];
    max?: number;
    /** Local branches checked out in another worktree: full ref name → worktree path. */
    worktrees?: ReadonlyMap<string, string>;
  }

  let { refs, max = 2, worktrees = NO_WORKTREES }: Props = $props();

  interface Badge {
    kind: RefLabelKind;
    name: string;
    current: boolean;
    /** A local branch checked out in another worktree. */
    wt: boolean;
    /** Remote branches merged into this local badge (`origin/x`). */
    remotes: string[];
    title: string;
  }

  /** Path of the worktree a label stands for (detached HEAD) or its local branch is checked out in. */
  function pathOf(r: RefLabel): string | undefined {
    if (r.kind === 'worktree') return r.fullName;
    return r.kind === 'local' ? worktrees.get(r.fullName) : undefined;
  }

  function toBadges(labels: RefLabel[]): Badge[] {
    const all: Badge[] = labels.map((r) => {
      const path = pathOf(r);
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

  const badges = $derived(toBadges(refs));
  const shown = $derived(badges.length > max ? badges.slice(0, max) : badges);
  const more = $derived(badges.length - shown.length);
  /** Every ref of the row, with the path of those tied to a worktree. */
  const moreTitle = $derived(
    more > 0
      ? refs
          .map((r) => {
            const path = pathOf(r);
            return path ? `${r.name} · ${path}` : r.name;
          })
          .join('\n')
      : '',
  );
</script>

<span class="refs">
  {#each shown as b}
    <span class="badge badge-{b.kind}" class:current={b.current} title={b.title}>
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
    background: var(--bg-3);
    color: var(--text-2);
  }
</style>
