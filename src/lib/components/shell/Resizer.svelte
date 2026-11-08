<!--
  Vertical splitter between two panes: a 1px line with a 6px hit area. Drag (pointer capture,
  one update per animation frame) to resize the pane on `side`; double-click resets to defaultValue.
-->
<script lang="ts">
  import { onMount } from 'svelte';

  interface Props {
    /** Pane width in px. */
    value: number;
    min: number;
    max: number;
    defaultValue: number;
    /** Which side of the splitter the resized pane is on. */
    side: 'left' | 'right';
  }

  let { value = $bindable(), min, max, defaultValue, side }: Props = $props();

  let dragging = $state(false);
  let pointerId = -1;
  let startX = 0;
  let startValue = 0;
  let lastX = 0;
  let frame = 0;

  function clamp(v: number): number {
    return Math.round(Math.max(min, Math.min(Math.max(min, max), v)));
  }

  function apply() {
    frame = 0;
    const dx = lastX - startX;
    const next = clamp(startValue + (side === 'left' ? dx : -dx));
    if (next !== value) value = next;
  }

  function onpointerdown(e: PointerEvent) {
    if (e.button !== 0 || dragging) return;
    if (e.currentTarget instanceof HTMLElement) e.currentTarget.setPointerCapture(e.pointerId);
    pointerId = e.pointerId;
    startX = lastX = e.clientX;
    startValue = value;
    dragging = true;
    document.documentElement.classList.add('col-resizing');
  }

  function onpointermove(e: PointerEvent) {
    if (!dragging || e.pointerId !== pointerId) return;
    lastX = e.clientX;
    if (!frame) frame = requestAnimationFrame(apply);
  }

  function stop(e: PointerEvent) {
    if (!dragging || e.pointerId !== pointerId) return;
    if (frame) {
      cancelAnimationFrame(frame);
      lastX = e.type === 'pointerup' ? e.clientX : lastX;
      apply();
    }
    dragging = false;
    pointerId = -1;
    document.documentElement.classList.remove('col-resizing');
  }

  /** Back to the default preference; `max` is only what fits right now (the layout clamps the rest). */
  function ondblclick() {
    value = Math.round(Math.max(min, defaultValue));
  }

  // Unmounted mid-drag (pane toggled off, window shrunk): drop the frame and the global cursor.
  onMount(() => () => {
    if (frame) cancelAnimationFrame(frame);
    frame = 0;
    if (dragging) document.documentElement.classList.remove('col-resizing');
  });
</script>

<!-- Pointer-only splitter; mousedown is cancelled so a drag never moves keyboard focus. -->
<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
<div
  class="resizer"
  class:dragging
  role="separator"
  aria-orientation="vertical"
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
</style>
