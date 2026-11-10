<!--
  Repository actions over the center pane, at the height of the details panel's bar (GitKraken's
  toolbar as the reference): Pull · Push · Branch · Stash · Pop, centered, each an icon with its short
  name. Pull / Push carry the checked-out branch's behind / ahead counts; right-click on Pull offers
  Fetch and Pull with rebase, on Pop the stash to pop. One operation runs at a time (repo.ops.busy):
  its button shows a spinner and the others disable; one without a button here (a commit's menu, the
  WIP banner: checkout, reset, cherry-pick / revert, tag) shows a spinner at the right end. Branch opens
  the branch-name input on HEAD's row.
-->
<script lang="ts">
  import Archive from '@lucide/svelte/icons/archive';
  import ArchiveArrowUp from '@lucide/svelte/icons/archive-arrow-up';
  import ArrowDownToLine from '@lucide/svelte/icons/arrow-down-to-line';
  import ArrowUpFromLine from '@lucide/svelte/icons/arrow-up-from-line';
  import CloudDownload from '@lucide/svelte/icons/cloud-download';
  import GitBranchPlus from '@lucide/svelte/icons/git-branch-plus';
  import GitPullRequestArrow from '@lucide/svelte/icons/git-pull-request-arrow';
  import { getRepo } from '../../state/repo.svelte';
  import { showContextMenu, type MenuEntry } from '../../state/ui.svelte';
  import { stashLabel } from '../sidebar/tree';
  import Spinner from '../common/Spinner.svelte';

  /** Stashes listed in Pop's right-click menu. */
  const MAX_STASH_MENU = 12;

  const repo = getRepo();
  const ops = repo.ops;

  const busy = $derived(ops.busy);
  /** An operation that has no button here is running. */
  const otherBusy = $derived(busy === 'checkout' || busy === 'reset' || busy === 'pick' || busy === 'tag');
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

  function pullMenu(e: MouseEvent): void {
    const items: MenuEntry[] = [
      { label: 'Fetch all', icon: CloudDownload, disabled: !canFetch, action: () => void ops.fetch() },
      { separator: true },
      { label: 'Pull', icon: ArrowDownToLine, disabled: !canPull, action: () => void ops.pull(false) },
      { label: 'Pull with rebase', icon: GitPullRequestArrow, disabled: !canPull, action: () => void ops.pull(true) },
    ];
    showContextMenu(e, items);
  }

  function popMenu(e: MouseEvent): void {
    if (stashes.length === 0) {
      e.preventDefault();
      return;
    }
    const items: MenuEntry[] = stashes.slice(0, MAX_STASH_MENU).map((s) => ({
      label: `${s.name} · ${stashLabel(s.message)}`,
      icon: ArchiveArrowUp,
      disabled: !!busy,
      action: () => void ops.pop(s),
    }));
    showContextMenu(e, items);
  }
</script>

<div class="toolbar" role="toolbar" aria-label="Repository actions">
  <!-- mousedown is cancelled on every button: the keyboard focus stays on the graph -->
  <button
    class="tb"
    class:off={!canPull}
    aria-disabled={!canPull}
    title={pullTitle}
    onmousedown={(e) => e.preventDefault()}
    onclick={() => canPull && void ops.pull(false)}
    oncontextmenu={pullMenu}
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
  <button
    class="tb"
    class:off={!canPop}
    aria-disabled={!canPop}
    title={popTitle}
    onmousedown={(e) => e.preventDefault()}
    onclick={() => canPop && popTarget && void ops.pop(popTarget)}
    oncontextmenu={popMenu}
  >
    {#if busy === 'pop' || busy === 'apply' || busy === 'drop'}
      <Spinner size={14} />
    {:else}
      <ArchiveArrowUp size={15} strokeWidth={1.75} />
    {/if}
    <span class="lbl">Pop</span>
    {#if stashes.length > 0}<span class="n">{stashes.length}</span>{/if}
  </button>
  {#if otherBusy}
    <span class="busy"><Spinner size={14} /></span>
  {/if}
</div>

<style>
  .toolbar {
    position: relative;
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
  /* Not `disabled`: a right-click still opens the button's menu (fetch without an upstream). */
  .tb.off {
    opacity: 0.4;
  }
  .n {
    font-family: var(--font-mono);
    font-size: var(--fs-xs);
    color: var(--text-2);
    font-variant-numeric: tabular-nums;
  }
  /* Out of the centered row, so the buttons stay where they are while it shows. */
  .busy {
    position: absolute;
    top: 0;
    right: 12px;
    bottom: 0;
    display: flex;
    align-items: center;
  }
  .sep {
    flex: none;
    width: 1px;
    height: 16px;
    margin: 0 6px;
    background: var(--border);
  }
</style>
