<!--
  A graph row's ref labels expanded: every badge of the row with its full name, one under the other,
  laid over the collapsed group (RefBadges opens it after a short hover; the first item lands on the
  first badge). One instance in App, above the pages. It flips up when it would run off the window and
  closes when the pointer leaves it, a list scrolls, a wheel over it would not scroll its own list, a
  key goes down or the pointer goes down elsewhere.
  Its items are the row's labels: a branch's or a tag's lights up under the pointer and opens that
  ref's menu on a right-click (the opener's `onmenu`). While that menu is up the popover stays, with
  the item lit, wherever the pointer goes; it closes with the menu unless the pointer is back on it.
-->
<script module lang="ts">
  import { contextMenu } from '../../state/ui.svelte';
  import type { Badge, RefMenuOpener } from './RefBadges.svelte';

  /** Padding around the items (matches the stylesheet): the first item's box lands on the first badge. */
  const PAD = 2;
  /** Clearance from the window edges. */
  const MARGIN = 4;
  /** Share of the window height the list may take before it scrolls. */
  const MAX_SHARE = 0.6;
  /** The pointer may take this long to cross from the group into the popover. */
  const CLOSE_MS = 80;

  export const refPopover = $state({
    open: false,
    /**
     * The RefBadges instance that opened it (its hover keeps it alive): a symbol of its own. Not an
     * object: this state would hand back a proxy of it, which is never the object it was given.
     */
    owner: null as symbol | null,
    left: 0,
    top: 0,
    /** At least the group's width: the popover covers the group, so leaving one means entering the other. */
    minWidth: 0,
    maxWidth: 0,
    maxHeight: 0,
    items: [] as Badge[],
    /** Opens the menu of an item's branch or tag (the opener's); null: no item has one. */
    onmenu: null as RefMenuOpener | null,
    /** The menu opened from an item — its `contextMenu.seq` — and that item's ref (full name). */
    menuSeq: -1,
    menuRef: '',
  });

  let closeTimer: ReturnType<typeof setTimeout> | undefined;
  /** The pointer is over the popover. */
  let inside = false;

  /** The menu opened from one of the items is up: it holds the popover open. */
  function menuUp(): boolean {
    return contextMenu.open && contextMenu.seq === refPopover.menuSeq;
  }

  /** Open for `owner` over `anchor` (the collapsed group's rect), or above it when it would run off the window. */
  export function showRefPopover(owner: symbol, anchor: DOMRect, items: Badge[], onmenu?: RefMenuOpener): void {
    clearTimeout(closeTimer);
    closeTimer = undefined;
    const h = Math.min(items.length * anchor.height + 2 * PAD, Math.floor(window.innerHeight * MAX_SHARE));
    let top = anchor.top - PAD;
    if (top + h > window.innerHeight - MARGIN) {
      // Bottom-aligned with the group; still too tall: as low as it fits (the group stays covered).
      const above = anchor.bottom + PAD - h;
      top = above >= MARGIN ? above : Math.max(MARGIN, window.innerHeight - MARGIN - h);
    }
    const left = Math.max(MARGIN, anchor.left - PAD);
    inside = false;
    refPopover.owner = owner;
    refPopover.left = left;
    refPopover.top = top;
    refPopover.minWidth = anchor.width + 2 * PAD;
    refPopover.maxWidth = Math.max(refPopover.minWidth, window.innerWidth - MARGIN - left);
    refPopover.maxHeight = h;
    refPopover.items = items;
    refPopover.onmenu = onmenu ?? null;
    refPopover.menuSeq = -1;
    refPopover.menuRef = '';
    refPopover.open = true;
  }

  export function hideRefPopover(): void {
    clearTimeout(closeTimer);
    closeTimer = undefined;
    inside = false;
    refPopover.open = false;
    refPopover.owner = null;
    refPopover.items = [];
    refPopover.onmenu = null;
    refPopover.menuSeq = -1;
    refPopover.menuRef = '';
  }

  /**
   * The pointer left the group or the popover: close unless it is inside the other within CLOSE_MS —
   * or the menu of an item is up by then, which keeps the popover until it closes.
   */
  export function closeRefPopoverSoon(): void {
    clearTimeout(closeTimer);
    closeTimer = setTimeout(() => {
      closeTimer = undefined;
      if (!menuUp()) hideRefPopover();
    }, CLOSE_MS);
  }

  export function cancelRefPopoverClose(): void {
    clearTimeout(closeTimer);
    closeTimer = undefined;
  }

  const MODIFIERS = new Set(['Shift', 'Meta', 'Alt', 'Control', 'CapsLock']);
</script>

<script lang="ts">
  import { untrack } from 'svelte';
  import Check from '@lucide/svelte/icons/check';
  import Cloud from '@lucide/svelte/icons/cloud';
  import Folder from '@lucide/svelte/icons/folder';
  import { KIND_ICONS } from './RefBadges.svelte';
  import { hasRefMenu } from './refmenu';

  let panel: HTMLDivElement | undefined = $state();

  /** The menu of one of the items is up: that item stays lit, and no other answers the pointer. */
  const held = $derived(contextMenu.open && contextMenu.seq === refPopover.menuSeq);

  /** Right-click on an item: the menu of its branch or tag, if it has one. */
  function onMenu(e: MouseEvent, b: Badge): void {
    const open = refPopover.onmenu;
    if (!open || !hasRefMenu(b.kind)) return;
    const seq = open(e, b);
    if (seq == null) return;
    refPopover.menuSeq = seq;
    refPopover.menuRef = b.fullName;
  }

  function onEnter(): void {
    inside = true;
    cancelRefPopoverClose();
  }

  function onLeave(): void {
    inside = false;
    closeRefPopoverSoon();
  }

  // The menu went: the popover goes with it, unless the pointer came back to it.
  let wasHeld = false;
  $effect(() => {
    const now = held;
    const gone = wasHeld && !now;
    wasHeld = now;
    if (gone && !inside) untrack(hideRefPopover);
  });

  // While open: rows scrolling under it, a key (not a modifier alone: ⌘-clicks start with one), a press
  // elsewhere, the window losing focus or resizing all close it. Its own scrolling does not. Under the
  // menu of an item, keys and presses are the menu's, and it decides when both go.
  $effect(() => {
    if (!refPopover.open) return;
    const onScroll = (e: Event) => {
      if (e.target !== panel && !menuUp()) hideRefPopover();
    };
    const onKey = (e: KeyboardEvent) => {
      if (!MODIFIERS.has(e.key) && !menuUp()) hideRefPopover();
    };
    const onDown = (e: PointerEvent) => {
      if (!(e.target instanceof Node && panel?.contains(e.target)) && !menuUp()) hideRefPopover();
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

  // The popover lies over the rows but outside their list, and WebKit scrolls the scroller under the
  // pointer: a wheel over it would scroll nothing. A wheel its own list cannot take (sideways, or past
  // either end) closes it, so the rest of the gesture scrolls the rows under it.
  $effect(() => {
    const el = panel;
    if (!el) return;
    const onWheel = (e: WheelEvent) => {
      const dy = e.deltaY;
      if (Math.abs(e.deltaX) > Math.abs(dy)) {
        hideRefPopover();
        return;
      }
      if (dy === 0) return;
      const room = dy > 0 ? el.scrollHeight - el.clientHeight - el.scrollTop >= 1 : el.scrollTop >= 1;
      if (!room) hideRefPopover();
    };
    el.addEventListener('wheel', onWheel, { passive: true });
    return () => el.removeEventListener('wheel', onWheel);
  });
</script>

{#if refPopover.open}
  <!-- A press on it must not take the keyboard focus from the graph's list: nothing here can hold it. -->
  <div
    class="pop"
    class:held
    bind:this={panel}
    role="presentation"
    aria-hidden="true"
    onmousedown={(e) => e.preventDefault()}
    style:left="{refPopover.left}px"
    style:top="{refPopover.top}px"
    style:min-width="{refPopover.minWidth}px"
    style:max-width="{refPopover.maxWidth}px"
    style:max-height="{refPopover.maxHeight}px"
    onpointerenter={onEnter}
    onpointerleave={onLeave}
  >
    {#each refPopover.items as b}
      {@const Kind = b.wt ? Folder : KIND_ICONS[b.kind]}
      <div
        class="item"
        class:current={b.current}
        class:tail={!!Kind}
        class:act={refPopover.onmenu != null && hasRefMenu(b.kind)}
        class:ctx={held && b.fullName === refPopover.menuRef}
        role="presentation"
        title={b.title !== b.name ? b.title : undefined}
        oncontextmenu={(e) => onMenu(e, b)}
      >
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
    background: var(--ref-label-bg);
    color: var(--ref-label-fg);
    box-shadow: var(--shadow);
    font-size: var(--fs-sm);
  }
  .item {
    flex: none;
    display: flex;
    align-items: center;
    gap: 4px;
    height: var(--ref-label-h);
    padding: 0 6px;
    border-radius: var(--radius-xs);
    line-height: var(--ref-label-h);
    white-space: nowrap;
  }
  .item.current {
    background: var(--ref-label-current-bg);
    color: var(--text-0);
  }
  /* An item with a menu of its own under the pointer, and the one whose menu is up: lit like its badge
     in the column. While a menu is up (`held`) only that menu's item is. */
  .pop:not(.held) .item.act:hover,
  .item.ctx {
    background-image: linear-gradient(var(--ref-label-hover), var(--ref-label-hover));
  }
  /* The kind icons and the end of an item holding them, as in the column badges. */
  .item.tail {
    padding-right: var(--ref-icon-inset);
  }
  .item :global(svg) {
    flex: none;
  }
  .item :global(svg.kind) {
    width: var(--ref-icon-size);
    height: var(--ref-icon-size);
  }
  .nm {
    overflow: hidden;
    text-overflow: ellipsis;
    min-width: 0;
  }
</style>
