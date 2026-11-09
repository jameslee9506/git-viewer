<!--
  Splitter between two panes: a 1px line with a 6px hit area. Vertical between side-by-side panes
  (`side` left / right: the pane's width) or horizontal between stacked panes (`side` top / bottom:
  the pane's height). Drag (pointer capture, one update per animation frame) to resize the pane on
  `side`; double-click resets to defaultValue.
-->
<script lang="ts">
  import { onMount } from 'svelte';

  interface Props {
    /** Pane width (left / right) or height (top / bottom) in px. */
    value: number;
    min: number;
    max: number;
    defaultValue: number;
    /** Which side of the splitter the resized pane is on. */
    side: 'left' | 'right' | 'top' | 'bottom';
  }

  let { value = $bindable(), min, max, defaultValue, side }: Props = $props();

  /** A horizontal line between stacked panes (the pointer's y moves it). */
  const horizontal = $derived(side === 'top' || side === 'bottom');

  let dragging = $state(false);
  let pointerId = -1;
  let start = 0;
  let startValue = 0;
  let last = 0;
  let frame = 0;
  /** The html class carrying the drag cursor (col- or row-resizing) while dragging. */
  let cursorClass = '';

  function clamp(v: number): number {
    return Math.round(Math.max(min, Math.min(Math.max(min, max), v)));
  }

  function pos(e: PointerEvent): number {
    return horizontal ? e.clientY : e.clientX;
  }

  function apply() {
    frame = 0;
    const d = last - start;
    const next = clamp(startValue + (side === 'left' || side === 'top' ? d : -d));
    if (next !== value) value = next;
  }

  function onpointerdown(e: PointerEvent) {
    if (e.button !== 0 || dragging) return;
    if (e.currentTarget instanceof HTMLElement) e.currentTarget.setPointerCapture(e.pointerId);
    pointerId = e.pointerId;
    start = last = pos(e);
    startValue = value;
    dragging = true;
    cursorClass = horizontal ? 'row-resizing' : 'col-resizing';
    document.documentElement.classList.add(cursorClass);
  }

  function onpointermove(e: PointerEvent) {
    if (!dragging || e.pointerId !== pointerId) return;
    last = pos(e);
    if (!frame) frame = requestAnimationFrame(apply);
  }

  function stop(e: PointerEvent) {
    if (!dragging || e.pointerId !== pointerId) return;
    if (frame) {
      cancelAnimationFrame(frame);
      last = e.type === 'pointerup' ? pos(e) : last;
      apply();
    }
    dragging = false;
    pointerId = -1;
    document.documentElement.classList.remove(cursorClass);
  }

  /** Back to the default preference; `max` is only what fits right now (the layout clamps the rest). */
  function ondblclick() {
    value = Math.round(Math.max(min, defaultValue));
  }

  // Unmounted mid-drag (pane toggled off, window shrunk): drop the frame and the global cursor.
  onMount(() => () => {
    if (frame) cancelAnimationFrame(frame);
    frame = 0;
    if (dragging) document.documentElement.classList.remove(cursorClass);
  });
</script>

<!-- Pointer-only splitter; mousedown is cancelled so a drag never moves keyboard focus. -->
<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
<div
  class="resizer"
  class:horizontal
  class:dragging
  role="separator"
  aria-orientation={horizontal ? 'horizontal' : 'vertical'}
  aria-valuenow={value}
  aria-valuemin={min}
  aria-valuemax={max}
  {onpointerdown}
  onmousedown={(e) => e.preventDefault()}
  {onpointermove}
  onpointerup={stop}
  onpointercancel={stop}
  onlostpointercapture={stop}
  {ondblclick}
></div>

<style>
  .resizer {
    position: relative;
    z-index: 5;
    width: 1px;
    min-width: 1px;
    height: 100%;
    background: var(--border);
    transition: background var(--dur);
  }
  .resizer::before {
    content: '';
    position: absolute;
    top: 0;
    bottom: 0;
    left: -3px;
    width: 6px;
    cursor: col-resize;
  }
  .resizer.horizontal {
    width: 100%;
    min-width: 0;
    height: 1px;
    min-height: 1px;
  }
  .resizer.horizontal::before {
    top: -3px;
    bottom: auto;
    left: 0;
    right: 0;
    width: auto;
    height: 6px;
    cursor: row-resize;
  }
  .resizer:hover {
    background: var(--border-strong);
  }
  .resizer.dragging {
    background: var(--accent);
    transition: none;
  }

  :global(html.col-resizing),
  :global(html.col-resizing *) {
    cursor: col-resize !important;
  }
  :global(html.row-resizing),
  :global(html.row-resizing *) {
    cursor: row-resize !important;
  }
</style>
