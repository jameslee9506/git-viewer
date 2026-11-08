<!--
  Lanes SVG for the rendered window of the graph list. Lives in the
  VirtualList overlay, one row above row `start`; about ten batched paths plus one shape per node.
  Colors come from CSS classes only (WebKit ignores var() in SVG presentation attributes).
-->
<script lang="ts">
  import { getRepo } from '../../state/repo.svelte';
  import { buildLanes, hasHeadLabel, MAX_LANES, naturalWidth, ROW_H } from './geometry';

  interface Props {
    /** First rendered row. */
    start: number;
    /** End of the rendered rows (exclusive). */
    end: number;
    /** Graph column width in px. */
    width: number;
  }

  let { start, end, width }: Props = $props();

  const graph = getRepo().graph;

  const model = $derived.by(() => {
    const total = graph.total;
    const head = graph.summary?.headRow ?? null;
    return buildLanes(
      (i) => graph.row(i),
      start,
      Math.min(end, total),
      total,
      width,
      // Previous-generation placeholder rows may sit at shifted indices: trust their labels instead.
      (i, row) => (graph.isFresh(i) ? i === head : hasHeadLabel(row)),
    );
  });
  const maxLanes = $derived(graph.summary?.maxLanes ?? 0);
  /** Lanes run past the right edge: the column is narrower than they need, or there are more than
   *  MAX_LANES (the widest column still cuts the rest off). */
  const fade = $derived(maxLanes > MAX_LANES || naturalWidth(maxLanes) > width);
  const sel = $derived(graph.selectedRow);
  const cmp = $derived(graph.compareRow);
</script>

{#if model.paths.length || model.nodes.length}
  <svg
    class="graph"
    class:fade
    width={width}
    height={model.height}
    style:top="{-ROW_H}px"
    aria-hidden="true"
  >
    {#each model.paths as p (p.cls)}
      <path class={p.cls} d={p.d} />
    {/each}
    {#each model.nodes as n (n.row)}
      {@const on = n.row === sel}
      {@const g = on ? 0.5 : 0}
      {#if n.kind === 'stash'}
        <rect
          class="h c{n.color}"
          class:sel={on}
          class:cmp={n.row === cmp}
          x={n.x - 4 - g}
          y={n.y - 4 - g}
          width={8 + 2 * g}
          height={8 + 2 * g}
          rx="1.5"
        />
      {:else if n.kind === 'wip'}
        <circle class="h d wip" class:sel={on} cx={n.x} cy={n.y} r={5 + g} />
      {:else if n.kind === 'head'}
        <circle class="h ring c{n.color}" class:sel={on} class:cmp={n.row === cmp} cx={n.x} cy={n.y} r={5.5 + g} />
        <circle class="f dot c{n.color}" cx={n.x} cy={n.y} r="2.5" />
      {:else}
        <circle
          class="f c{n.color}"
          class:sel={on}
          class:cmp={n.row === cmp}
          cx={n.x}
          cy={n.y}
          r={(n.kind === 'merge' ? 3 : 4) + g}
        />
      {/if}
    {/each}
  </svg>
{/if}

<style>
  .graph {
    position: absolute;
    left: 0;
    z-index: 1;
    overflow: hidden;
    pointer-events: none;
  }
  .graph.fade {
    -webkit-mask-image: linear-gradient(to right, var(--bg-1) calc(100% - 12px), transparent);
    mask-image: linear-gradient(to right, var(--bg-1) calc(100% - 12px), transparent);
  }
  .c0 {
    color: var(--lane-0);
  }
  .c1 {
    color: var(--lane-1);
  }
  .c2 {
    color: var(--lane-2);
  }
  .c3 {
    color: var(--lane-3);
  }
  .c4 {
    color: var(--lane-4);
  }
  .c5 {
    color: var(--lane-5);
  }
  .c6 {
    color: var(--lane-6);
  }
  .c7 {
    color: var(--lane-7);
  }
  .d {
    color: var(--lane-wip);
  }
  path {
    fill: none;
    stroke: currentColor;
    stroke-width: 2;
    stroke-linecap: butt;
  }
  /* Period 26/6: divides the row height, so dashes keep their phase while the window moves. */
  path.d {
    stroke-dasharray: 2 2.3333;
  }
  path.o {
    stroke-opacity: 0.4;
  }
  /* filled nodes: lane color with a background halo */
  .f {
    fill: currentColor;
    stroke: var(--bg-1);
    stroke-width: 1.5;
    paint-order: stroke;
  }
  .f.dot {
    stroke: none;
  }
  /* hollow nodes: background fill, lane-colored outline */
  .h {
    fill: var(--bg-1);
    stroke: currentColor;
    stroke-width: 1.5;
  }
  .h.ring {
    stroke-width: 2;
  }
  .h.wip {
    stroke-dasharray: 2 2;
  }
  /* keep halos/holes in the selected row's background */
  .f.sel,
  .f.cmp {
    stroke: var(--bg-active);
  }
  .h.sel,
  .h.cmp {
    fill: var(--bg-active);
  }
  :global(.gv-focused) .f.sel {
    stroke: var(--bg-active-strong);
  }
  :global(.gv-focused) .h.sel {
    fill: var(--bg-active-strong);
  }
</style>
