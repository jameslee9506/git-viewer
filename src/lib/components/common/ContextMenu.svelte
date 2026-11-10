<!--
  The app's single context menu (state: `contextMenu` in state/ui.svelte.ts, opened with showContextMenu).
  Opens at the pointer, flipped/clamped to stay inside the window. An entry with `children` opens them
  as a submenu beside it (left of the menu where the window ends): on hover, → or Enter; the pointer may
  cut across other entries on its way into it. ↑↓ Home End move within the level the keyboard is in,
  Enter/Space run an entry when the key is released (a held key neither runs what it just opened nor
  reaches what the entry opens or the view behind), ← leaves a submenu, Esc/Tab close; closes on
  outside pointerdown, outside wheel (the user scrolling; a scroll made by code never closes it),
  resize and window blur.
-->
<script lang="ts">
  import ChevronRight from '@lucide/svelte/icons/chevron-right';
  import { contextMenu, hideContextMenu, type MenuEntry } from '../../state/ui.svelte';
  import { isComposing } from '../../util/keys';

  type Item = Exclude<MenuEntry, { separator: true }>;
  type Parent = Item & { children: MenuEntry[] };

  interface Point {
    x: number;
    y: number;
  }

  /** Gap kept between a menu and the window edges. */
  const MARGIN = 4;
  /** A menu's padding (as in the stylesheet): a submenu's first entry lines up with its parent entry. */
  const PAD = 4;
  /** How far a submenu overlaps its parent menu, so the pointer crosses no gap between them. */
  const OVERLAP = 3;
  /**
   * While the pointer heads for the open submenu, an entry it crosses takes over only once the pointer
   * has rested this long. A rest that long also starts the pointer's direction anew.
   */
  const AIM_MS = 200;
  /** Slack above and below the submenu for that heading, in px. */
  const AIM_SLACK = 24;

  let el: HTMLDivElement | undefined = $state();
  let subEl: HTMLDivElement | undefined = $state();
  /** Highlighted entry of the menu (-1: none). */
  let active = $state(-1);
  /** The entry whose submenu is open (-1: none). */
  let sub = $state(-1);
  /** Highlighted entry of the submenu (-1: none). */
  let subActive = $state(-1);
  /** The keyboard moves within the submenu: a key opened it, or the pointer is in it. */
  let inSub = $state(false);

  /** The pointer's last positions over the menu, oldest first: its direction. */
  let trail: Point[] = [];
  /** When the pointer last moved over the menu (`performance.now()`). */
  let movedAt = 0;
  /** The crossed entry waiting for the pointer to rest or turn away (-1: none), and its timer: see `settle`. */
  let waiting = -1;
  let aimTimer: ReturnType<typeof setTimeout> | undefined;
  /** The key that closed the menu: its repeats are swallowed until it is released (see below). */
  let closedBy: string | null = null;
  /**
   * The entry picked with Enter / Space, until that key comes up: it runs then. A key that is still
   * down would reach what the entry opens: a native confirmation (Hard reset, Drop, Abort) takes the
   * key's repeats for a press of its default button.
   */
  let armed: { code: string; entry: Item } | null = null;

  const isItem = (e: MenuEntry | undefined): e is Item => !!e && !('separator' in e);
  const enabled = (e: MenuEntry | undefined): e is Item => isItem(e) && !e.disabled;
  /** The entry has a submenu: children that are more than separators. */
  const hasSub = (e: Item): e is Parent => !!e.children?.some(isItem);
  /** An enabled entry with a submenu to open. */
  const isParent = (e: MenuEntry | undefined): e is Parent => enabled(e) && hasSub(e);

  /** `list` without leading, trailing or doubled separators. */
  function tidy(list: MenuEntry[]): MenuEntry[] {
    const out: MenuEntry[] = [];
    for (const e of list) {
      if (isItem(e)) out.push(e);
      else if (out.length > 0 && isItem(out[out.length - 1])) out.push(e);
    }
    if (out.length > 0 && !isItem(out[out.length - 1])) out.pop();
    return out;
  }

  const entries = $derived(tidy(contextMenu.items));
  const subEntries = $derived.by(() => {
    const e = entries[sub];
    return isParent(e) ? tidy(e.children) : [];
  });
  const hasIcons = $derived(entries.some((e) => isItem(e) && !!e.icon));
  const subHasIcons = $derived(subEntries.some((e) => isItem(e) && !!e.icon));

  function run(entry: MenuEntry | undefined): void {
    if (!enabled(entry)) return;
    hideContextMenu();
    entry.action?.();
  }

  /** Enter / Space went down on `entry`: it runs once that key (`code`) comes up, see `armed`. */
  function arm(code: string, entry: MenuEntry | undefined): void {
    armed = enabled(entry) ? { code, entry } : null;
  }

  /** The next enabled entry of `list` from `from` in `dir`, wrapping; from none, the end it starts at. */
  function step(list: MenuEntry[], from: number, dir: 1 | -1): number {
    const n = list.length;
    let i = from;
    for (let k = 0; k < n; k++) {
      i = i < 0 ? (dir > 0 ? 0 : n - 1) : (i + dir + n) % n;
      if (enabled(list[i])) return i;
    }
    return -1;
  }

  // ───────────── submenu ─────────────

  /** No crossed entry waits any longer. */
  function unwait(): void {
    clearTimeout(aimTimer);
    waiting = -1;
  }

  function closeSub(): void {
    unwait();
    sub = -1;
    subActive = -1;
    inSub = false;
  }

  /** Open the submenu of entry i; `byKey` moves the keyboard into it, onto its first entry. */
  function openSub(i: number, byKey: boolean): void {
    const e = entries[i];
    if (!isParent(e)) return;
    unwait();
    active = i;
    sub = i;
    inSub = byKey;
    subActive = byKey ? tidy(e.children).findIndex((c) => enabled(c)) : -1;
  }

  /** ←: back to the submenu's entry. */
  function leaveSub(): void {
    const i = sub;
    closeSub();
    active = i;
  }

  // ───────────── keyboard ─────────────

  /** Move the menu's highlight; a submenu open under another entry closes. */
  function moveTo(i: number): void {
    active = i;
    if (sub >= 0 && sub !== i) closeSub();
  }

  function onKey(e: KeyboardEvent): void {
    if (isComposing(e)) return;
    // The menu is modal for the keyboard: nothing underneath reacts while it is open.
    e.preventDefault();
    e.stopPropagation();
    // Another key calls off the entry that waits for its own to come up.
    if (armed && e.code !== armed.code) armed = null;
    const deep = sub >= 0 && inSub;
    switch (e.code) {
      case 'ArrowDown':
      case 'ArrowUp': {
        const dir = e.code === 'ArrowDown' ? 1 : -1;
        if (deep) subActive = step(subEntries, subActive, dir);
        else moveTo(step(entries, active, dir));
        break;
      }
      case 'Home':
      case 'End': {
        const dir = e.code === 'Home' ? 1 : -1;
        if (deep) subActive = step(subEntries, -1, dir);
        else moveTo(step(entries, -1, dir));
        break;
      }
      case 'ArrowRight':
        if (!deep) openSub(active, true);
        break;
      case 'ArrowLeft':
        if (deep) leaveSub();
        break;
      case 'Enter':
      case 'NumpadEnter':
      case 'Space':
        // Once per press: the repeats of a held key would pick the first entry of the submenu it opened.
        if (e.repeat) break;
        if (deep) {
          if (subActive >= 0) arm(e.code, subEntries[subActive]);
          else hideContextMenu();
        } else if (active < 0) hideContextMenu();
        else if (isParent(entries[active])) openSub(active, true);
        else arm(e.code, entries[active]);
        break;
      case 'Escape':
      case 'Tab':
        hideContextMenu();
        break;
    }
    if (!contextMenu.open) closedBy = e.code;
  }

  /** The key of the picked entry came up: now it runs. */
  function onKeyUp(e: KeyboardEvent): void {
    const picked = armed;
    if (!picked || e.code !== picked.code) return;
    armed = null;
    e.preventDefault();
    e.stopPropagation();
    run(picked.entry);
  }

  // ───────────── pointer ─────────────

  /** Note where the pointer is (`mouseenter` and the `mousemove` after it report the same spot once). */
  function track(e: MouseEvent): void {
    const last = trail[trail.length - 1];
    if (last && last.x === e.clientX && last.y === e.clientY) return;
    const now = performance.now();
    // After a rest the direction is that of the new move alone, from where the pointer rested.
    if (last && now - movedAt > AIM_MS) trail = [last];
    movedAt = now;
    trail.push({ x: e.clientX, y: e.clientY });
    if (trail.length > 3) trail.shift();
  }

  /**
   * Whether a pointer that went from `from` to `to` heads for `box` (the open submenu): `to` lies in
   * the triangle between `from` and the box's near edge, stretched by AIM_SLACK up and down.
   */
  function heading(from: Point, to: Point, box: DOMRect): boolean {
    // The near edge: the left one of a box right of the pointer, the right one of a box left of it.
    const edge = box.left > from.x ? box.left : box.right < from.x ? box.right : null;
    if (edge == null) return false;
    // The share of the way to that edge the pointer covered (negative: it moved away).
    const done = Math.min(1, (to.x - from.x) / (edge - from.x));
    if (!(done > 0)) return false;
    const top = from.y + (box.top - AIM_SLACK - from.y) * done;
    const bottom = from.y + (box.bottom + AIM_SLACK - from.y) * done;
    return to.y >= top && to.y <= bottom;
  }

  /** The pointer's last move heads into the open submenu. */
  function aiming(): boolean {
    const box = subEl?.getBoundingClientRect();
    const from = trail[0];
    const to = trail[trail.length - 1];
    return !!box && !!from && !!to && heading(from, to, box);
  }

  /** Entry i takes the highlight (and shows its submenu, closing another one). */
  function hover(i: number): void {
    const e = entries[i];
    active = enabled(e) ? i : -1;
    inSub = false;
    subActive = -1;
    sub = isParent(e) ? i : -1;
  }

  /**
   * The pointer is over entry i: it takes the highlight at once, unless the pointer is crossing it on
   * the way into the open submenu — then only once the pointer turns away (asked again on every move,
   * see `move`) or has rested there for AIM_MS.
   */
  function settle(i: number): void {
    unwait();
    const rest = AIM_MS - (performance.now() - movedAt);
    if (sub >= 0 && i !== sub && rest > 0 && aiming()) {
      waiting = i;
      aimTimer = setTimeout(() => settle(i), rest);
    } else hover(i);
  }

  function enter(i: number, e: MouseEvent): void {
    track(e);
    settle(i);
  }

  /** The pointer moved over the menu: a waiting entry is asked again. */
  function move(e: MouseEvent): void {
    track(e);
    if (waiting >= 0) settle(waiting);
  }

  function click(i: number): void {
    const e = entries[i];
    if (isParent(e)) openSub(i, false);
    else run(e);
  }

  /** The pointer left the menu, into the submenu or away: an open submenu's entry keeps the highlight. */
  function leave(): void {
    unwait();
    trail = [];
    active = sub;
  }

  function enterSub(): void {
    unwait();
    inSub = true;
    active = sub;
  }

  // ───────────── placement ─────────────

  // Position at the pointer (flip left/up when it would overflow), reset the highlight and the submenu.
  $effect(() => {
    const menu = el;
    if (!contextMenu.open || !menu) return;
    const { x, y } = contextMenu;
    void entries;
    active = -1;
    closeSub();
    trail = [];
    armed = null;
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

  // The submenu beside its entry: right of the menu, else on the roomier side — narrowed to the room
  // there rather than laid over the menu, where its entries would come under the pointer. Its first
  // entry is level with the parent entry, moved up as far as the window asks.
  $effect(() => {
    const menu = subEl;
    const parent = el;
    if (!contextMenu.open || sub < 0 || !menu || !parent) return;
    void subEntries;
    const item = parent.querySelector<HTMLElement>(`[data-i="${sub}"]`);
    if (!item) return;
    const p = parent.getBoundingClientRect();
    const r = item.getBoundingClientRect();
    menu.style.left = '0px';
    menu.style.top = '0px';
    menu.style.minWidth = '';
    menu.style.maxWidth = '';
    menu.style.maxHeight = '';
    const vw = window.innerWidth;
    const vh = window.innerHeight;
    // The room beside the menu, the overlap included.
    const roomRight = vw - MARGIN - (p.right - OVERLAP);
    const roomLeft = p.left + OVERLAP - MARGIN;
    let w = menu.offsetWidth;
    const onRight = w <= roomRight || roomRight >= roomLeft;
    const room = Math.max(0, onRight ? roomRight : roomLeft);
    if (w > room) {
      menu.style.minWidth = '0';
      menu.style.maxWidth = `${room}px`;
      w = menu.offsetWidth;
    }
    const h = menu.offsetHeight;
    // clientTop: the border.
    let top = r.top - PAD - menu.clientTop;
    top = Math.max(MARGIN, Math.min(top, vh - h - MARGIN));
    if (h > vh - 2 * MARGIN) {
      top = MARGIN;
      menu.style.maxHeight = `${vh - 2 * MARGIN}px`;
    }
    menu.style.left = `${onRight ? p.right - OVERLAP : p.left + OVERLAP - w}px`;
    menu.style.top = `${top}px`;
  });

  // Nothing to show (no items, only separators): close instead of holding the keyboard invisibly.
  $effect(() => {
    if (contextMenu.open && entries.length === 0) hideContextMenu();
  });

  // Closed with a crossed entry still waiting, or a picked one whose key is still down: nothing is
  // left for them to act on.
  $effect(() => {
    if (contextMenu.open) return;
    unwait();
    armed = null;
  });

  // A key that closed the menu (Enter on an entry, Esc) may still be down: its repeats would reach the
  // view behind (the graph opens the selection on Enter). They are swallowed until the key is released
  // or another one goes down. Registered once, ahead of the listeners below.
  $effect(() => {
    const onDown = (e: KeyboardEvent) => {
      if (closedBy == null) return;
      if (e.repeat && e.code === closedBy) {
        e.preventDefault();
        e.stopPropagation();
      } else closedBy = null;
    };
    const onUp = () => {
      closedBy = null;
    };
    window.addEventListener('keydown', onDown, true);
    window.addEventListener('keyup', onUp, true);
    return () => {
      window.removeEventListener('keydown', onDown, true);
      window.removeEventListener('keyup', onUp, true);
    };
  });

  // While open: keyboard (capture phase, ahead of every other handler) and the close triggers.
  // Scrolling closes it only through the user's gesture (wheel / trackpad, momentum included), never
  // through a `scroll` event: the right-click itself may swap a pane whose content then resets its
  // scroll position from code (file history, tree), which must not dismiss the menu just opened.
  // A scrollbar drag starts with a pointerdown, and keyboard scrolling is blocked by onKey.
  $effect(() => {
    if (!contextMenu.open) return;
    const inside = (t: EventTarget | null) => t instanceof Node && (!!el?.contains(t) || !!subEl?.contains(t));
    const onOutside = (e: Event) => {
      if (!inside(e.target)) hideContextMenu();
    };
    window.addEventListener('keydown', onKey, true);
    window.addEventListener('keyup', onKeyUp, true);
    window.addEventListener('pointerdown', onOutside, true);
    window.addEventListener('wheel', onOutside, { capture: true, passive: true });
    window.addEventListener('resize', hideContextMenu);
    window.addEventListener('blur', hideContextMenu);
    return () => {
      window.removeEventListener('keydown', onKey, true);
      window.removeEventListener('keyup', onKeyUp, true);
      window.removeEventListener('pointerdown', onOutside, true);
      window.removeEventListener('wheel', onOutside, true);
      window.removeEventListener('resize', hideContextMenu);
      window.removeEventListener('blur', hideContextMenu);
    };
  });
</script>

{#snippet body(entry: Item, icons: boolean)}
  {#if icons}
    <span class="icon">
      {#if entry.icon}<entry.icon size={14} strokeWidth={1.75} />{/if}
    </span>
  {/if}
  <span class="label ellipsis">{entry.label}{#if entry.note}<span class="note">{entry.note}</span>{/if}</span>
  {#if entry.hint}<span class="hint kbd">{entry.hint}</span>{/if}
  {#if hasSub(entry)}
    <span class="more"><ChevronRight size={14} strokeWidth={1.75} /></span>
  {/if}
{/snippet}

<!-- Entries are never `disabled`: the pointer events of a disabled button are not to be relied on, and
     a disabled entry under the pointer has to clear the highlight and close a submenu like any other. -->
{#if contextMenu.open && entries.length > 0}
  <div
    class="menu"
    bind:this={el}
    role="menu"
    tabindex="-1"
    onmousedown={(e) => e.preventDefault()}
    oncontextmenu={(e) => e.preventDefault()}
    onmousemove={move}
    onmouseleave={leave}
    onscroll={closeSub}
  >
    {#each entries as entry, i (i)}
      {#if 'separator' in entry}
        <div class="sep" role="separator"></div>
      {:else}
        <button
          class="item"
          class:active={i === active}
          class:off={entry.disabled}
          role="menuitem"
          tabindex="-1"
          data-i={i}
          aria-disabled={entry.disabled}
          aria-haspopup={hasSub(entry) ? 'menu' : undefined}
          aria-expanded={hasSub(entry) ? i === sub : undefined}
          onmouseenter={(e) => enter(i, e)}
          onclick={() => click(i)}
        >
          {@render body(entry, hasIcons)}
        </button>
      {/if}
    {/each}
  </div>
  {#if sub >= 0 && subEntries.length > 0}
    <div
      class="menu"
      bind:this={subEl}
      role="menu"
      tabindex="-1"
      onmousedown={(e) => e.preventDefault()}
      oncontextmenu={(e) => e.preventDefault()}
      onmouseenter={enterSub}
      onmouseleave={() => (subActive = -1)}
    >
      {#each subEntries as entry, j (j)}
        {#if 'separator' in entry}
          <div class="sep" role="separator"></div>
        {:else}
          <button
            class="item"
            class:active={j === subActive}
            class:off={entry.disabled}
            role="menuitem"
            tabindex="-1"
            aria-disabled={entry.disabled}
            onmouseenter={() => (subActive = entry.disabled ? -1 : j)}
            onclick={() => run(entry)}
          >
            {@render body(entry, subHasIcons)}
          </button>
        {/if}
      {/each}
    </div>
  {/if}
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
    background: var(--bg-menu-hover);
  }
  .item.off {
    color: var(--text-3);
  }

  .icon {
    display: inline-flex;
    width: 14px;
    flex: none;
    color: var(--text-1);
  }

  .label {
    flex: 1 1 auto;
  }
  /* What the entry does, where its name alone does not tell: muted, after the name. */
  .note {
    margin-left: 8px;
    color: var(--text-2);
  }

  .hint {
    flex: none;
    margin-left: 16px;
  }

  /* The submenu mark, at the entry's right end. */
  .more {
    display: inline-flex;
    flex: none;
    margin: 0 -4px 0 8px;
    color: var(--text-2);
  }
  .item.off .icon,
  .item.off .note,
  .item.off .more {
    color: var(--text-3);
  }

  .sep {
    height: 1px;
    margin: 4px 6px;
    background: var(--border);
  }
</style>
