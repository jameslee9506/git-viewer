<!-- Shared change glyphs for file rows, change summaries and the graph's WIP counts. -->
<script module lang="ts">
  import ArrowRight from '@lucide/svelte/icons/arrow-right';
  import Copy from '@lucide/svelte/icons/copy';
  import FileType from '@lucide/svelte/icons/file-type';
  import TriangleExclamationPoint from '@lucide/svelte/icons/triangle-exclamation-point';
  import type { ChangeStatus } from '../../types';

  const OUTLINE_ICONS: Partial<Record<ChangeStatus, typeof Copy>> = {
    R: ArrowRight,
    C: Copy,
    T: FileType,
    U: TriangleExclamationPoint,
  };
</script>

<script lang="ts">
  let { status }: { status: ChangeStatus } = $props();

  const OutlineIcon = $derived(OUTLINE_ICONS[status]);
  const added = $derived(status === 'A' || status === '?');
  /** Filled 12px glyphs: pencil, plus and minus (the minus is the plus's bar). */
  const filled = $derived(status === 'M' || status === 'D' || added);
</script>

<span class="status-icon" class:filled aria-hidden="true">
  {#if status === 'M'}
    <svg viewBox="0 0 16 16" fill="currentColor">
      <path d="M2.5 10.5 9.25 3.75l3 3L5.5 13.5l-4 1zM10 3l1.5-1.5a1.4 1.4 0 0 1 2 0l1 1a1.4 1.4 0 0 1 0 2L13 6z" />
    </svg>
  {:else if added}
    <svg viewBox="0 0 16 16" fill="currentColor">
      <path d="M6.5 1.5h3v5h5v3h-5v5h-3v-5h-5v-3h5z" />
    </svg>
  {:else if status === 'D'}
    <svg viewBox="0 0 16 16" fill="currentColor">
      <path d="M1.5 6.5h13v3h-13z" />
    </svg>
  {:else if OutlineIcon}
    <OutlineIcon size="100%" strokeWidth={2} />
  {:else}
    {status}
  {/if}
</span>

<style>
  .status-icon {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    flex: none;
    width: var(--status-icon-slot);
    height: var(--status-icon-slot);
  }
  .status-icon.filled {
    width: var(--status-icon-size);
    height: var(--status-icon-size);
  }
  .status-icon > :global(svg) {
    display: block;
    width: 100%;
    height: 100%;
  }
</style>
