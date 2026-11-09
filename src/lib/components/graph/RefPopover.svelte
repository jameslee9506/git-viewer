<!--
  A graph row's ref labels expanded: every badge of the row with its full name, one under the other,
  laid over the collapsed group (RefBadges opens it after a short hover; the first item lands on the
  first badge). One instance in App, above the pages. It flips up when it would run off the window and
  closes when the pointer leaves it, a list scrolls, a key goes down or the pointer goes down elsewhere.
-->
<script module lang="ts">
  import type { Badge } from './RefBadges.svelte';

  /** Padding around the items (matches the stylesheet): the first item's box lands on the first badge. */
  const PAD = 2;
  /** Item height: the column badges' height (matches the stylesheet). */
  const ITEM_H = 20;
  /** Clearance from the window edges. */
  const MARGIN = 4;
  /** Share of the window height the list may take before it scrolls. */
  const MAX_SHARE = 0.6;
  /** The pointer may take this long to cross from the group into the popover. */
  const CLOSE_MS = 80;

  export const refPopover = $state({
    open: false,
    /** The RefBadges instance that opened it (its hover keeps it alive). */
    owner: null as object | null,
    left: 0,
    top: 0,
    /** At least the group's width: the popover covers the group, so leaving one means entering the other. */
    minWidth: 0,
    maxWidth: 0,
    maxHeight: 0,
    items: [] as Badge[],
  });

  let closeTimer: ReturnType<typeof setTimeout> | undefined;

  /** Open for `owner` over `anchor` (the collapsed group's rect), or above it when it would run off the window. */
  export function showRefPopover(owner: object, anchor: DOMRect, items: Badge[]): void {
    clearTimeout(closeTimer);
    closeTimer = undefined;
    const h = Math.min(items.length * ITEM_H + 2 * PAD, Math.floor(window.innerHeight * MAX_SHARE));
    let top = anchor.top - PAD;
    if (top + h > window.innerHeight - MARGIN) {
      // Bottom-aligned with the group; still too tall: as low as it fits (the group stays covered).
      const above = anchor.bottom + PAD - h;
      top = above >= MARGIN ? above : Math.max(MARGIN, window.innerHeight - MARGIN - h);
    }
    const left = Math.max(MARGIN, anchor.left - PAD);
    refPopover.owner = owner;
    refPopover.left = left;
    refPopover.top = top;
    refPopover.minWidth = anchor.width + 2 * PAD;
    refPopover.maxWidth = Math.max(refPopover.minWidth, window.innerWidth - MARGIN - left);
    refPopover.maxHeight = h;
    refPopover.items = items;
    refPopover.open = true;
  }

  export function hideRefPopover(): void {
    clearTimeout(closeTimer);
    closeTimer = undefined;
    refPopover.open = false;
    refPopover.owner = null;
    refPopover.items = [];
  }

  /** The pointer left the group or the popover: close unless it is inside the other within CLOSE_MS. */
  export function closeRefPopoverSoon(): void {
    clearTimeout(closeTimer);
    closeTimer = setTimeout(hideRefPopover, CLOSE_MS);
  }

  export function cancelRefPopoverClose(): void {
    clearTimeout(closeTimer);
    closeTimer = undefined;
  }

  const MODIFIERS = new Set(['Shift', 'Meta', 'Alt', 'Control', 'CapsLock']);
</script>

<script lang="ts">
  import Check from '@lucide/svelte/icons/check';
  import Cloud from '@lucide/svelte/icons/cloud';
  import Folder from '@lucide/svelte/icons/folder';
  import { KIND_ICONS } from './RefBadges.svelte';

  let panel: HTMLDivElement | undefined = $state();

  // While open: rows scrolling under it, a key (not a modifier alone: ⌘-clicks start with one), a press
  // elsewhere, the window losing focus or resizing all close it. Its own scrolling does not.
  $effect(() => {
    if (!refPopover.open) return;
    const onScroll = (e: Event) => {
      if (e.target !== panel) hideRefPopover();
    };
    const onKey = (e: KeyboardEvent) => {
      if (!MODIFIERS.has(e.key)) hideRefPopover();
    };
    const onDown = (e: PointerEvent) => {
      if (!(e.target instanceof Node && panel?.contains(e.target))) hideRefPopover();
    };
    const hide = () => hideRefPopover();
    window.addEventListener('scroll', onScroll, { capture: true, passive: true });
    window.addEventListener('keydown', onKey, true);
    window.addEventListener('pointerdown', onDown, true);
    window.addEventListener('blur', hide);
    window.addEventListener('resize', hide);
    return () => {
      window.removeEventListener('scroll', onScroll, true);
      window.removeEventListener('keydown', onKey, true);
      window.removeEventListener('pointerdown', onDown, true);
      window.removeEventListener('blur', hide);
      window.removeEventListener('resize', hide);
    };
  });
</script>

{#if refPopover.open}
  <div
    class="pop"
    bind:this={panel}
    role="presentation"
    aria-hidden="true"
    style:left="{refPopover.left}px"
    style:top="{refPopover.top}px"
    style:min-width="{refPopover.minWidth}px"
    style:max-width="{refPopover.maxWidth}px"
    style:max-height="{refPopover.maxHeight}px"
    onpointerenter={cancelRefPopoverClose}
    onpointerleave={closeRefPopoverSoon}
  >
    {#each refPopover.items as b}
      {@const Kind = b.wt ? Folder : KIND_ICONS[b.kind]}
      <div class="item" class:current={b.current} title={b.title !== b.name ? b.title : undefined}>
        {#if b.current}
          <Check size={12} strokeWidth={2} />
        {/if}
        <span class="nm">{b.name}</span>
        {#if Kind}
          <Kind size={12} strokeWidth={2} />
        {/if}
        {#if b.remotes.length}
          <Cloud size={12} strokeWidth={2} />
        {/if}
      </div>
    {/each}
  </div>
{/if}

<style>
  /* Above the pages (the sticky cells, time marks), below the context menu (1000). Looks like one tall
     label: the column badges' colors and corners. */
  .pop {
    position: fixed;
    z-index: 800;
    display: flex;
    flex-direction: column;
    padding: 2px;
    overflow: hidden auto;
    border-radius: var(--radius-xs);
    background: var(--label-bg);
    color: var(--label-fg);
    box-shadow: var(--shadow);
    font-size: var(--fs-sm);
  }
  .item {
    flex: none;
    display: flex;
    align-items: center;
    gap: 4px;
    height: 20px;
    padding: 0 6px;
    line-height: 20px;
    white-space: nowrap;
  }
  .item.current {
    color: var(--text-0);
    font-weight: 600;
  }
  .item :global(svg) {
    flex: none;
  }
  .nm {
    overflow: hidden;
    text-overflow: ellipsis;
    min-width: 0;
  }
</style>
