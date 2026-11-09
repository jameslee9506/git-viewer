<script lang="ts">
  import Download from '@lucide/svelte/icons/download';
  import RotateCw from '@lucide/svelte/icons/rotate-cw';
  import { updater } from '../../state/updater.svelte';
  import Spinner from '../common/Spinner.svelte';

  const title = $derived.by(() => {
    switch (updater.phase) {
      case 'downloading':
        return `Downloading ${updater.version}${updater.progress === null ? '' : ` · ${updater.progress}%`}`;
      case 'installing':
        return `Installing ${updater.version}`;
      case 'restarting':
        return `Restarting to ${updater.version}`;
      case 'restart':
        return `Restart to finish updating to ${updater.version}${updater.error ? ` · ${updater.error}` : ''}`;
      default:
        return updater.error
          ? `Retry update to ${updater.version} · ${updater.error}`
          : `Update to ${updater.version} and restart`;
    }
  });
</script>

{#if updater.visible}
  <button
    class="icon-btn update"
    {title}
    aria-label={title}
    aria-busy={updater.busy}
    disabled={updater.busy}
    onclick={() => void updater.install()}
  >
    {#if updater.busy}
      <Spinner />
    {:else if updater.phase === 'restart'}
      <RotateCw />
    {:else}
      <Download />
    {/if}
    <span>
      {#if updater.phase === 'downloading'}
        {updater.progress === null ? '…' : `${updater.progress}%`}
      {:else if updater.phase === 'installing'}
        Installing
      {:else if updater.phase === 'restarting'}
        Restarting
      {:else if updater.phase === 'restart'}
        Restart
      {:else}
        Update
      {/if}
    </span>
  </button>
{/if}

<style>
  .update {
    width: auto;
    height: var(--list-h);
    gap: var(--sp-1);
    padding-inline: var(--sp-2);
    background: var(--accent-dim);
    color: var(--text-0);
    font-size: var(--fs-sm);
    font-variant-numeric: tabular-nums;
    white-space: nowrap;
  }
  .update:disabled {
    /* The control is locked, but its progress and tooltip should remain readable. */
    opacity: 1;
    pointer-events: auto;
  }
</style>
