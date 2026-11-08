<!--
  The app's single context menu (state: `contextMenu` in state/ui.svelte.ts, opened with showContextMenu).
  Opens at the pointer, flipped/clamped to stay inside the window. ↑↓ Home End move, Enter/Space run,
  Esc/Tab close; closes on outside pointerdown, outside wheel (the user scrolling; a scroll made by
  code never closes it), resize and window blur.
-->
<script lang="ts">
  import { contextMenu, hideContextMenu, type MenuEntry } from '../../state/ui.svelte';
  import { isComposing } from '../../util/keys';

  type Item = Exclude<MenuEntry, { separator: true }>;

  /** Gap kept between the menu and the window edges. */
  const MARGIN = 4;

  let el: HTMLDivElement | undefined = $state();
  let active = $state(-1);

  const isItem = (e: MenuEntry): e is Item => !('separator' in e);
  const enabled = (e: MenuEntry | undefined): e is Item => !!e && isItem(e) && !e.disabled;

  /** Entries without leading, trailing or doubled separators. */
  const entries = $derived.by(() => {
    const out: MenuEntry[] = [];
    for (const e of contextMenu.items) {
      if (isItem(e)) out.push(e);
      else if (out.length > 0 && isItem(out[out.length - 1])) out.push(e);
    }
    if (out.length > 0 && !isItem(out[out.length - 1])) out.pop();
    return out;
  });

  const hasIcons = $derived(entries.some((e) => isItem(e) && !!e.icon));

  function run(entry: MenuEntry | undefined) {
    if (!enabled(entry)) return;
    hideContextMenu();
    entry.action();
  }

  /** Move the highlight to the next enabled entry in `dir`, wrapping; from none, start at an end. */
  function move(dir: 1 | -1) {
    const n = entries.length;
    let i = active;
    for (let k = 0; k < n; k++) {
      i = i < 0 ? (dir > 0 ? 0 : n - 1) : (i + dir + n) % n;
      if (enabled(entries[i])) {
        active = i;
        return;
      }
    }
  }

  function onKey(e: KeyboardEvent) {
    if (isComposing(e)) return;
    // The menu is modal for the keyboard: nothing underneath reacts while it is open.
    e.preventDefault();
    e.stopPropagation();
    switch (e.code) {
      case 'ArrowDown':
        move(1);
        break;
      case 'ArrowUp':
        move(-1);
        break;
      case 'Home':
        active = -1;
        move(1);
        break;
      case 'End':
        active = -1;
        move(-1);
        break;
      case 'Enter':
      case 'NumpadEnter':
      case 'Space':
        if (active >= 0) run(entries[active]);
        else hideContextMenu();
        break;
      case 'Escape':
      case 'Tab':
        hideContextMenu();
        break;
    }
  }

  // Position at the pointer (flip left/up when it would overflow), reset the highlight.
  $effect(() => {
    const menu = el;
    if (!contextMenu.open || !menu) return;
    const { x, y } = contextMenu;
    void entries;
    active = -1;
    // Measure at the origin: at a previous spot near an edge the menu would shrink-wrap narrower.
    menu.style.left = '0px';
    menu.style.top = '0px';
    menu.style.maxHeight = '';
    const w = menu.offsetWidth;
    const h = menu.offsetHeight;
    const vw = window.innerWidth;
    const vh = window.innerHeight;
    let left = x + w + MARGIN > vw ? x - w : x;
    let top = y + h + MARGIN > vh ? y - h : y;
    left = Math.max(MARGIN, Math.min(left, vw - w - MARGIN));
    top = Math.max(MARGIN, Math.min(top, vh - h - MARGIN));
    if (h > vh - 2 * MARGIN) {
      top = MARGIN;
      menu.style.maxHeight = `${vh - 2 * MARGIN}px`;
    }
    menu.style.left = `${left}px`;
    menu.style.top = `${top}px`;
  });

  // Nothing to show (no items, only separators): close instead of holding the keyboard invisibly.
  $effect(() => {
    if (contextMenu.open && entries.length === 0) hideContextMenu();
  });

  // While open: keyboard (capture phase, ahead of every other handler) and the close triggers.
  // Scrolling closes it only through the user's gesture (wheel / trackpad, momentum included), never
  // through a `scroll` event: the right-click itself may swap a pane whose content then resets its
  // scroll position from code (file history, tree), which must not dismiss the menu just opened.
  // A scrollbar drag starts with a pointerdown, and keyboard scrolling is blocked by onKey.
  $effect(() => {
    if (!contextMenu.open) return;
    const inside = (t: EventTarget | null) => !!el && t instanceof Node && el.contains(t);
    const onOutside = (e: Event) => {
      if (!inside(e.target)) hideContextMenu();
    };
    window.addEventListener('keydown', onKey, true);
    window.addEventListener('pointerdown', onOutside, true);
    window.addEventListener('wheel', onOutside, { capture: true, passive: true });
    window.addEventListener('resize', hideContextMenu);
    window.addEventListener('blur', hideContextMenu);
    return () => {
      window.removeEventListener('keydown', onKey, true);
      window.removeEventListener('pointerdown', onOutside, true);
      window.removeEventListener('wheel', onOutside, true);
      window.removeEventListener('resize', hideContextMenu);
      window.removeEventListener('blur', hideContextMenu);
    };
  });
</script>

{#if contextMenu.open && entries.length > 0}
  <div
    class="menu"
    bind:this={el}
    role="menu"
    tabindex="-1"
    onmousedown={(e) => e.preventDefault()}
    oncontextmenu={(e) => e.preventDefault()}
    onmouseleave={() => (active = -1)}
  >
    {#each entries as entry, i (i)}
      {#if 'separator' in entry}
        <div class="sep" role="separator"></div>
      {:else}
        <button
          class="item"
          class:active={i === active}
          role="menuitem"
          tabindex="-1"
          disabled={entry.disabled}
          onmouseenter={() => (active = entry.disabled ? -1 : i)}
          onclick={() => run(entry)}
        >
          {#if hasIcons}
            <span class="icon">
              {#if entry.icon}<entry.icon size={14} strokeWidth={1.75} />{/if}
            </span>
          {/if}
          <span class="label ellipsis">{entry.label}</span>
          {#if entry.hint}<span class="hint kbd">{entry.hint}</span>{/if}
        </button>
      {/if}
    {/each}
  </div>
{/if}

<style>
  .menu {
    position: fixed;
    left: 0;
    top: 0;
    z-index: 1000;
    min-width: 168px;
    max-width: 360px;
    padding: 4px;
    overflow-x: hidden;
    overflow-y: auto;
    background: var(--bg-float);
    border: 1px solid var(--border-strong);
    border-radius: var(--radius);
    box-shadow: var(--shadow);
    font-size: var(--fs-sm);
    outline: none;
  }

  .item {
    display: flex;
    align-items: center;
    gap: 8px;
    width: 100%;
    height: 24px;
    padding: 0 8px;
    border-radius: var(--radius-sm);
    color: var(--text-0);
    text-align: left;
    white-space: nowrap;
  }
  .item.active {
    background: var(--bg-hover);
  }
  .item:disabled {
    color: var(--text-3);
  }

  .icon {
    display: inline-flex;
    width: 14px;
    flex: none;
    color: var(--text-1);
  }
  .item:disabled .icon {
    color: var(--text-3);
  }

  .label {
    flex: 1 1 auto;
  }

  .hint {
    flex: none;
    margin-left: 16px;
  }

  .sep {
    height: 1px;
    margin: 4px 6px;
    background: var(--border);
  }
</style>
