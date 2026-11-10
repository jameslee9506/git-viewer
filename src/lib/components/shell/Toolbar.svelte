<!--
  Repository actions over the center pane, at the height of the details panel's bar (GitKraken's
  toolbar as the reference): Pull · Push · Branch · Stash · Pop, centered, each an icon with its short
  name. Pull / Push carry the checked-out branch's behind / ahead counts. Pull and Pop are split
  buttons: the chevron at their right opens their menu under them (Pull: Fetch all, Pull, Pull with
  rebase; Pop: the stash to pop), as a right-click on either part does at the pointer. One operation
  runs at a time (repo.ops.busy): its button shows a spinner and the others disable; the footer names
  it, also one without a button here (a commit's menu, the WIP banner). Branch opens the branch-name
  input on HEAD's row. In a narrow center pane (a small or zoomed window) the buttons close up, and
  narrower still they keep their icons only.
-->
<script lang="ts">
  import Archive from '@lucide/svelte/icons/archive';
  import ArchiveArrowUp from '@lucide/svelte/icons/archive-arrow-up';
  import ArrowDownToLine from '@lucide/svelte/icons/arrow-down-to-line';
  import ArrowUpFromLine from '@lucide/svelte/icons/arrow-up-from-line';
  import ChevronDown from '@lucide/svelte/icons/chevron-down';
  import CloudDownload from '@lucide/svelte/icons/cloud-download';
  import GitBranchPlus from '@lucide/svelte/icons/git-branch-plus';
  import GitPullRequestArrow from '@lucide/svelte/icons/git-pull-request-arrow';
  import { untrack } from 'svelte';
  import { getRepo } from '../../state/repo.svelte';
  import { contextMenu, hideContextMenu, showContextMenu, showMenu, type MenuEntry } from '../../state/ui.svelte';
  import { stashLabel } from '../sidebar/tree';
  import Spinner from '../common/Spinner.svelte';

  /** Stashes listed in Pop's menu. */
  const MAX_STASH_MENU = 12;
  /** Gap between a split button and the menu its chevron opens under it. */
  const MENU_GAP = 4;

  type Split = 'pull' | 'pop';

  const repo = getRepo();
  const ops = repo.ops;

  const busy = $derived(ops.busy);
  const status = $derived(repo.status);
  const head = $derived(status?.head ?? null);
  const onBranch = $derived(!!head?.branch && !head.detached && !head.unborn);
  const upstream = $derived(status?.upstream ?? null);
  const stashes = $derived(repo.refs?.stashes ?? []);
  /** A merge / rebase / … in progress: pulling or pushing into it is not offered. */
  const inOperation = $derived(!!status?.operation);

  const canPull = $derived(!busy && !!upstream && !inOperation);
  const canFetch = $derived(!busy && (repo.refs?.remoteList.length ?? 0) > 0);
  const canPush = $derived(!busy && onBranch && !inOperation && (!!upstream || ops.defaultRemote() != null));
  const canBranch = $derived(!busy && !!(status?.head.oid ?? repo.refs?.head.oid) && repo.graph.summary?.headRow != null);
  const canStash = $derived(!busy && repo.dirty && !inOperation);
  const canPop = $derived(!busy && stashes.length > 0);

  const pullTitle = $derived(upstream ? `Pull · ${upstream}` : 'Pull');
  const pushTitle = $derived.by(() => {
    if (upstream) return `Push · ${upstream}`;
    const remote = ops.defaultRemote();
    return remote && head?.branch ? `Push · ${remote}/${head.branch} (sets the upstream)` : 'Push';
  });
  /** Pop's target: the selected stash, else the newest one. */
  const popTarget = $derived.by(() => {
    const sel = repo.selection;
    return (sel.kind === 'stash' && stashes.find((s) => s.oid === sel.oid)) || stashes[0] || null;
  });
  const popTitle = $derived(popTarget ? `Pop · ${stashLabel(popTarget.message)}` : 'Pop');

  function pullItems(): MenuEntry[] {
    return [
      { label: 'Fetch all', icon: CloudDownload, disabled: !canFetch, action: () => void ops.fetch() },
      { separator: true },
      { label: 'Pull', icon: ArrowDownToLine, disabled: !canPull, action: () => void ops.pull(false) },
      { label: 'Pull with rebase', icon: GitPullRequestArrow, disabled: !canPull, action: () => void ops.pull(true) },
    ];
  }

  function popItems(): MenuEntry[] {
    return stashes.slice(0, MAX_STASH_MENU).map((s) => ({
      label: `${s.name} · ${stashLabel(s.message)}`,
      icon: ArchiveArrowUp,
      disabled: !!busy,
      action: () => void ops.pop(s),
    }));
  }

  const items = (of: Split): MenuEntry[] => (of === 'pull' ? pullItems() : popItems());

  /** The split buttons' groups: a chevron's menu hangs from its group's left end. */
  const groups: Partial<Record<Split, HTMLElement>> = $state({});
  /** The menu a chevron opened: its `contextMenu.seq`, and whose it is. */
  let dropdown = $state.raw<{ seq: number; of: Split } | null>(null);
  /** The split button whose chevron menu is up. */
  const open = $derived(dropdown && contextMenu.open && contextMenu.seq === dropdown.seq ? dropdown.of : null);

  /** A chevron: its menu under the split button, or away again when it is the one that shows. */
  function toggle(of: Split, e: MouseEvent): void {
    if (open === of) {
      hideContextMenu();
      return;
    }
    const group = groups[of];
    const list = items(of);
    if (!group || list.length === 0 || !(e.currentTarget instanceof Element)) return;
    const r = group.getBoundingClientRect();
    dropdown = { seq: showMenu({ x: r.left, y: r.bottom + MENU_GAP }, list, e.currentTarget), of };
  }

  /** The toolbar's width: its buttons are centered in it. */
  let width = $state(0);

  // A pane shown, hidden or dragged moves the buttons away from under a chevron's menu: it closes.
  // (Only the width is tracked: the effect must not run, and close, when a chevron opens its menu.)
  $effect(() => {
    void width;
    untrack(() => {
      if (open) hideContextMenu();
    });
  });

  /** A right-click on either part of a split button: the same menu, at the pointer. */
  function menu(of: Split, e: MouseEvent): void {
    const list = items(of);
    // Nothing to offer (no stash): no menu, the page's own neither.
    if (list.length === 0) e.preventDefault();
    else showContextMenu(e, list);
  }
</script>

<div class="toolbar" role="toolbar" aria-label="Repository actions" bind:clientWidth={width}>
  <!-- mousedown is cancelled on every button: the keyboard focus stays on the graph -->
  <div class="split" class:open={open === 'pull'} bind:this={groups.pull}>
    <button
      class="tb"
      class:off={!canPull}
      aria-disabled={!canPull}
      title={pullTitle}
      onmousedown={(e) => e.preventDefault()}
      onclick={() => canPull && void ops.pull(false)}
      oncontextmenu={(e) => menu('pull', e)}
    >
      {#if busy === 'pull' || busy === 'fetch'}
        <Spinner size={14} />
      {:else}
        <ArrowDownToLine size={15} strokeWidth={1.75} />
      {/if}
      <span class="lbl">Pull</span>
      {#if status && status.behind > 0}<span class="n">↓{status.behind}</span>{/if}
    </button>
    <button
      class="tb more"
      class:off={!canFetch && !canPull}
      title="Fetch all · Pull with rebase"
      aria-label="Fetch and pull options"
      aria-haspopup="menu"
      aria-expanded={open === 'pull'}
      onmousedown={(e) => e.preventDefault()}
      onclick={(e) => toggle('pull', e)}
      oncontextmenu={(e) => menu('pull', e)}
    >
      <ChevronDown size={12} strokeWidth={2} />
    </button>
  </div>
  <button
    class="tb"
    class:off={!canPush}
    aria-disabled={!canPush}
    title={pushTitle}
    onmousedown={(e) => e.preventDefault()}
    onclick={() => canPush && void ops.push()}
  >
    {#if busy === 'push'}
      <Spinner size={14} />
    {:else}
      <ArrowUpFromLine size={15} strokeWidth={1.75} />
    {/if}
    <span class="lbl">Push</span>
    {#if status && status.ahead > 0}<span class="n">↑{status.ahead}</span>{/if}
  </button>
  <span class="sep"></span>
  <button
    class="tb"
    class:off={!canBranch}
    aria-disabled={!canBranch}
    title="Branch · create at HEAD and switch to it"
    onmousedown={(e) => e.preventDefault()}
    onclick={() => canBranch && repo.promptBranchAtHead()}
  >
    {#if busy === 'branch'}
      <Spinner size={14} />
    {:else}
      <GitBranchPlus size={15} strokeWidth={1.75} />
    {/if}
    <span class="lbl">Branch</span>
  </button>
  <button
    class="tb"
    class:off={!canStash}
    aria-disabled={!canStash}
    title="Stash · all changes, untracked files included"
    onmousedown={(e) => e.preventDefault()}
    onclick={() => canStash && void ops.stash()}
  >
    {#if busy === 'stash'}
      <Spinner size={14} />
    {:else}
      <Archive size={15} strokeWidth={1.75} />
    {/if}
    <span class="lbl">Stash</span>
  </button>
  <div class="split" class:open={open === 'pop'} bind:this={groups.pop}>
    <button
      class="tb"
      class:off={!canPop}
      aria-disabled={!canPop}
      title={popTitle}
      onmousedown={(e) => e.preventDefault()}
      onclick={() => canPop && popTarget && void ops.pop(popTarget)}
      oncontextmenu={(e) => menu('pop', e)}
    >
      {#if busy === 'pop' || busy === 'apply' || busy === 'drop'}
        <Spinner size={14} />
      {:else}
        <ArchiveArrowUp size={15} strokeWidth={1.75} />
      {/if}
      <span class="lbl">Pop</span>
      {#if stashes.length > 0}<span class="n">{stashes.length}</span>{/if}
    </button>
    <button
      class="tb more"
      class:off={!canPop}
      title="Pop another stash"
      aria-label="Stashes to pop"
      aria-haspopup="menu"
      aria-expanded={open === 'pop'}
      onmousedown={(e) => e.preventDefault()}
      onclick={(e) => toggle('pop', e)}
      oncontextmenu={(e) => menu('pop', e)}
    >
      <ChevronDown size={12} strokeWidth={2} />
    </button>
  </div>
</div>

<style>
  .toolbar {
    flex: none;
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 2px;
    height: var(--toolbar-h);
    min-width: 0;
    padding: 0 8px;
    border-bottom: 1px solid var(--border);
    background: var(--bg-2);
    /* The buttons adapt to the room the center pane leaves (the rules at the end). */
    container-type: inline-size;
  }
  .tb {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    height: 26px;
    padding: 0 10px;
    border-radius: var(--radius-sm);
    color: var(--text-1);
    font-size: var(--fs-sm);
    white-space: nowrap;
    transition:
      background var(--dur),
      color var(--dur);
  }
  .tb :global(svg),
  .tb :global(.spinner) {
    flex: none;
    color: var(--text-2);
  }
  .tb:hover:not(.off) {
    background: var(--bg-hover);
    color: var(--text-0);
  }
  .tb:hover:not(.off) :global(svg) {
    color: var(--text-0);
  }
  /* Not `disabled`: the chevron and a right-click still open the button's menu (fetch without an upstream). */
  .tb.off {
    opacity: 0.4;
  }
  /* A split button: the action and, flush at its right, the chevron that opens its menu. */
  .split {
    display: inline-flex;
    align-items: center;
  }
  .split > .tb:first-child {
    padding-right: 6px;
    border-top-right-radius: 0;
    border-bottom-right-radius: 0;
  }
  .more {
    justify-content: center;
    width: 18px;
    padding: 0;
    border-top-left-radius: 0;
    border-bottom-left-radius: 0;
  }
  /* While its menu is up the chevron stays lit. */
  .split.open > .more {
    background: var(--bg-hover);
  }
  .split.open > .more :global(svg) {
    color: var(--text-0);
  }
  .n {
    font-family: var(--font-mono);
    font-size: var(--fs-xs);
    color: var(--text-2);
    font-variant-numeric: tabular-nums;
  }
  .sep {
    flex: none;
    width: 1px;
    height: 16px;
    margin: 0 6px;
    background: var(--border);
  }

  /* A narrow center pane: the buttons close up, so that the counts still fit at the pane's minimum
     width; narrower than that (a small window, zoomed in) they keep their icons, the names being in
     the tooltips. Widths are the toolbar's content box. */
  @container (max-width: 500px) {
    .tb {
      gap: 4px;
      padding: 0 6px;
    }
    .split > .tb:first-child {
      padding-right: 3px;
    }
    .more {
      width: 16px;
      padding: 0;
    }
    .sep {
      margin: 0 3px;
    }
  }
  @container (max-width: 400px) {
    .lbl {
      display: none;
    }
  }
</style>
