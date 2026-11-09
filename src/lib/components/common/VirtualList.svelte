<!--
  Fixed-row-height virtual scroller. Renders only rows in view (+overscan). A few rows may be taller
  (`tall` indices + `tallExtra`, e.g. the sidebar's section headers; geometry in rows.ts).
  - `row(i)` snippet renders row i (wrapped in a div of exactly its height).
  - `overlay(start, end)` snippet renders inside the translated window (absolute, top = row `start`),
    e.g. the graph lanes SVG.
  - Call exported scrollToIndex()/pageSize()/firstVisibleIndex()/getScrollTop()/setScrollTop() via
    bind:this.
  - Scroll positions are virtual: row i starts at i * rowHeight plus the extra height of the tall rows
    before it (`onscroll`, getScrollTop()). A list
    taller than MAX_PX scrolls a MAX_PX spacer in pages (see Scroller), so getViewport().scrollTop is
    then a physical position that does not convert to rows.
-->
<script module lang="ts">
  /**
   * Tallest spacer in CSS px. WebKit clamps layout sizes at 33,554,431 layout px (LayoutUnit is 1/64 px
   * in an int32) and page zoom (up to 1.6) multiplies CSS px, so a spacer of count * rowHeight would
   * cut the oldest rows of a huge history off. 8M stays far below the clamp at any zoom and keeps the
   * compositor's float32 offsets precise.
   */
  const MAX_PX = 8_000_000;
  /** Pages per spacer: a page is about MAX_PX / PAGES px of 1:1 scrolling. */
  const PAGES = 100;
  /** After the physical scrollTop was moved, how long a scroll event may still report a position from before. */
  const STALE_MS = 200;
  /** A scroll this soon after a wheel event is wheel scrolling (1:1) even when frames were coalesced… */
  const WHEEL_MS = 150;
  /** …unless it is this far (a jump: Home/End, the scrollbar). */
  const WHEEL_MAX = 20_000;

  /** The scrolling element (a stub with the same fields in tests). */
  interface ScrollBox {
    scrollTop: number;
    readonly clientHeight: number;
  }

  /**
   * Pages of a list taller than MAX_PX. Page p holds the virtual positions [p, p + 1) * size and shows
   * them at physical scrollTop = virtual - offsetOf(p). Offsets rise linearly from 0 (first page) to
   * `shift` (last page), so the ends of the list sit at the ends of the spacer.
   */
  interface Pages {
    /** Viewport height. */
    view: number;
    /** Virtual scroll range: content height minus viewport height. */
    span: number;
    /** Physical scroll range: MAX_PX minus viewport height. */
    range: number;
    /** span - range: the offset of the last page. */
    shift: number;
    /** Page count (≥ 2). */
    n: number;
    /** Page length (span / n). */
    size: number;
  }

  function pagesFor(total: number, el: ScrollBox): Pages | null {
    if (!(total > MAX_PX)) return null;
    const view = Math.max(0, Math.min(el.clientHeight, MAX_PX / 2));
    const span = total - view;
    const range = MAX_PX - view;
    // n ≥ span / range keeps every page's physical positions within [0, range]; PAGES times that
    // makes a page about range / PAGES long.
    const n = Math.max(2, Math.ceil((PAGES * span) / range));
    return { view, span, range, shift: span - range, n, size: span / n };
  }

  function pageAt(g: Pages, v: number): number {
    return Math.max(0, Math.min(g.n - 1, Math.floor(v / g.size)));
  }

  /** Whole px, so the rows stay on the pixel grid. */
  function offsetOf(g: Pages, p: number): number {
    return Math.round((g.shift * p) / (g.n - 1));
  }

  /** Page for a jump to physical y: one that holds y + its offset, nearest the proportional position. */
  function pageForY(g: Pages, y: number): number {
    let p = Math.max(0, Math.min(g.n - 1, Math.round((y * (g.n - 1)) / g.range)));
    for (let k = 0; k < 4; k++) {
      const q = pageAt(g, y + offsetOf(g, p));
      if (q === p) break;
      p += q > p ? 1 : -1;
    }
    return p;
  }

  /**
   * Virtual scroll position of a list: virtual = physical scrollTop + offset. The offset is 0 unless
   * the list is taller than MAX_PX; then it is the offset of the page in view (SlickGrid-style paging).
   * Wheel, keyboard and programmatic scrolling move 1:1, and entering another page moves the physical
   * scrollTop by the change of offset, so the content stays put. A jump (scrollbar drag, track click,
   * Home/End) keeps the physical position and takes the page nearest the proportional position.
   */
  class Scroller {
    /** Virtual scrollTop. */
    top = 0;
    /** Virtual minus physical scrollTop. */
    offset = 0;
    /** Physical scrollTop last seen or set. */
    #y = 0;
    #wheelAt = -Infinity;
    /** The last re-page during a scroll, while a position from before it may still arrive (see #place). */
    #moved: { from: number; to: number; offset: number; at: number } | null = null;

    wheel(now: number): void {
      this.#wheelAt = now;
    }

    /** Virtual scrollTop, also while a scroll event is pending. */
    current(el: ScrollBox): number {
      return el.scrollTop + this.offset;
    }

    /** A scroll event. */
    scrolled(el: ScrollBox, total: number, now: number): void {
      const y = el.scrollTop;
      const g = pagesFor(total, el);
      if (!g) {
        this.#moved = null;
        this.#set(y, 0);
        return;
      }
      const m = this.#moved;
      if (m && now - m.at < STALE_MS && Math.abs(y - m.from) < Math.abs(y - m.to)) {
        // Async scrolling reported a position from before that move: read it with the offset of then,
        // and show it in the current page (the move may still be on its way).
        const to = y + m.offset - this.offset;
        if (Math.abs(to - y) >= 0.5) el.scrollTop = to;
        this.#set(el.scrollTop, this.offset);
        return;
      }
      this.#moved = null;
      const dy = Math.abs(y - this.#y);
      if (dy > g.view && !(now - this.#wheelAt < WHEEL_MS && dy < WHEEL_MAX)) {
        this.#set(y, offsetOf(g, pageForY(g, y)));
        return;
      }
      this.#place(el, g, y + this.offset, now, true);
    }

    /** Scroll to virtual position v (clamped to the list). */
    scrollTo(el: ScrollBox, total: number, v: number, now: number): void {
      const g = pagesFor(total, el);
      if (g) {
        this.#place(el, g, Math.max(0, Math.min(g.span, v)), now, false);
        return;
      }
      this.#moved = null;
      el.scrollTop = Math.max(0, v);
      this.#set(el.scrollTop, 0);
    }

    /** The content or viewport height changed: keep the virtual position (in range). */
    resized(el: ScrollBox, total: number, now: number): void {
      if (this.offset === 0 && total <= MAX_PX) {
        // The browser keeps a plain list's scrollTop in range itself; pick up its clamp now.
        const max = Math.max(0, total - el.clientHeight);
        if (el.scrollTop > max) el.scrollTop = max;
        this.#set(el.scrollTop, 0);
        return;
      }
      this.scrollTo(el, total, this.top, now);
    }

    /**
     * Show virtual position v with the offset of its page. `scrolling`: re-paging during a scroll, the
     * one move that a position from before it may still follow. Such a position is told apart only
     * when the move is far longer than a scroll in STALE_MS; a programmatic move supersedes it.
     */
    #place(el: ScrollBox, g: Pages, v: number, now: number, scrolling: boolean): void {
      const offset = offsetOf(g, pageAt(g, Math.max(0, Math.min(g.span, v))));
      const from = el.scrollTop;
      const to = v - offset;
      if (Math.abs(to - from) >= 0.5) {
        const tell = scrolling && offset !== this.offset && Math.abs(to - from) > 4 * g.view;
        this.#moved = tell ? { from, to, offset: this.offset, at: now } : null;
        el.scrollTop = to;
      }
      this.#set(el.scrollTop, offset);
    }

    #set(y: number, offset: number): void {
      this.#y = y;
      this.offset = offset;
      this.top = y + offset;
    }
  }
</script>

<script lang="ts">
  import type { Snippet } from 'svelte';
  import { rowAt, rowFrom, rowHeightAt, rowTop, type RowMetrics } from './rows';

  const NO_TALL: readonly number[] = [];

  interface Props {
    count: number;
    rowHeight: number;
    /** Rows `tallExtra` px taller than `rowHeight`: their indices, ascending (few; e.g. section headers). */
    tall?: ArrayLike<number>;
    tallExtra?: number;
    overscan?: number;
    row: Snippet<[number]>;
    overlay?: Snippet<[number, number]>;
    /** Allow horizontal scrolling (rows keep intrinsic width). */
    horizontal?: boolean;
    /** Minimum scrollable content width in px when horizontal. */
    contentWidth?: number;
    class?: string;
    tabindex?: number;
    onrange?: (start: number, end: number) => void;
    /** Virtual scrollTop (row i starts at i * rowHeight) on every scroll event. */
    onscroll?: (scrollTop: number) => void;
    onkeydown?: (e: KeyboardEvent) => void;
  }

  let {
    count,
    rowHeight,
    tall = NO_TALL,
    tallExtra = 0,
    overscan = 8,
    row,
    overlay,
    horizontal = false,
    contentWidth = 0,
    class: klass = '',
    tabindex = -1,
    onrange,
    onscroll,
    onkeydown,
  }: Props = $props();

  let viewport: HTMLDivElement | undefined = $state();
  /** Virtual scrollTop (Scroller.top). */
  let top = $state(0);
  /** Virtual minus physical scrollTop (Scroller.offset): rows sit at i * rowHeight - offset in the spacer. */
  let offset = $state(0);
  let height = $state(0);
  const scroller = new Scroller();

  const metrics = $derived<RowMetrics>({ rowHeight, tall, extra: tallExtra });
  const total = $derived(rowTop(metrics, count));
  const scaled = $derived(total > MAX_PX);
  const start = $derived(Math.max(0, rowAt(metrics, top, count) - overscan));
  const end = $derived(Math.min(count, rowFrom(metrics, top + height, count) + overscan));
  const indices = $derived.by(() => {
    const out: number[] = [];
    for (let i = start; i < end; i++) out.push(i);
    return out;
  });

  function sync() {
    top = scroller.top;
    offset = scroller.offset;
  }

  $effect(() => {
    onrange?.(start, end);
  });

  $effect(() => {
    const el = viewport;
    if (!el) return;
    const ro = new ResizeObserver(() => {
      height = el.clientHeight;
    });
    ro.observe(el);
    height = el.clientHeight;
    return () => ro.disconnect();
  });

  // Keep the virtual position when the content or viewport height changes (scrollTop in range when
  // count shrinks; a paged list re-pages).
  $effect(() => {
    const el = viewport;
    if (!el) return;
    const t = total;
    void height;
    scroller.resized(el, t, performance.now());
    sync();
  });

  // Wheel input tells wheel scrolling from jumps on paged lists only.
  $effect(() => {
    const el = viewport;
    if (!el || !scaled) return;
    const onwheel = () => scroller.wheel(performance.now());
    el.addEventListener('wheel', onwheel, { passive: true });
    return () => el.removeEventListener('wheel', onwheel);
  });

  function handleScroll() {
    if (!viewport) return;
    scroller.scrolled(viewport, total, performance.now());
    sync();
    onscroll?.(top);
  }

  /** Scroll so that row i is visible. 'auto' scrolls minimally. */
  export function scrollToIndex(i: number, align: 'auto' | 'start' | 'center' | 'end' = 'auto') {
    const el = viewport;
    if (!el) return;
    const cur = scroller.current(el);
    const y = rowTop(metrics, i);
    const rh = rowHeightAt(metrics, i);
    const h = el.clientHeight;
    let target = cur;
    if (align === 'start') target = y;
    else if (align === 'end') target = y - h + rh;
    else if (align === 'center') target = y - (h - rh) / 2;
    else if (y < cur) target = y;
    else if (y + rh > cur + h) target = y - h + rh;
    scroller.scrollTo(el, total, target, performance.now());
    sync();
  }

  /** Number of fully visible rows. */
  export function pageSize(): number {
    return Math.max(1, Math.floor(height / rowHeight));
  }

  /** Virtual scrollTop: row i starts at i * rowHeight (plus the tall rows' extra height before it). */
  export function getScrollTop(): number {
    return viewport ? scroller.current(viewport) : top;
  }

  /** Scroll to a virtual scrollTop (as getScrollTop() returns it). */
  export function setScrollTop(value: number) {
    const el = viewport;
    if (!el) return;
    scroller.scrollTo(el, total, value, performance.now());
    sync();
  }

  /** The first fully visible row (0 when empty). */
  export function firstVisibleIndex(): number {
    return Math.max(0, Math.min(count - 1, rowFrom(metrics, getScrollTop(), count)));
  }

  export function getViewport(): HTMLDivElement | undefined {
    return viewport;
  }

  export function focus() {
    viewport?.focus({ preventScroll: true });
  }
</script>

<!-- svelte-ignore a11y_no_noninteractive_tabindex, a11y_no_static_element_interactions -->
<div
  class="vl {klass}"
  class:horizontal
  bind:this={viewport}
  onscroll={handleScroll}
  {onkeydown}
  {tabindex}
>
  <div
    class="vl-spacer"
    style:height="{scaled ? MAX_PX : total}px"
    style:min-width={horizontal && contentWidth ? `${contentWidth}px` : null}
  >
    <div class="vl-window" style:transform="translateY({rowTop(metrics, start) - offset}px)">
      {#if overlay}{@render overlay(start, end)}{/if}
      {#each indices as i (i)}
        <div class="vl-row" style:height="{rowHeightAt(metrics, i)}px">{@render row(i)}</div>
      {/each}
    </div>
  </div>
</div>

<style>
  .vl {
    position: relative;
    overflow-x: hidden;
    overflow-y: auto;
    outline: none;
    contain: strict;
    height: 100%;
    width: 100%;
  }
  .vl.horizontal {
    overflow-x: auto;
  }
  .vl-spacer {
    position: relative;
    min-width: 100%;
  }
  .vl-window {
    position: absolute;
    top: 0;
    left: 0;
    right: 0;
    will-change: transform;
  }
  .horizontal .vl-window {
    right: auto;
    min-width: 100%;
  }
  .vl-row {
    position: relative;
    overflow: hidden;
  }
  .horizontal .vl-row {
    overflow: visible;
  }
</style>
