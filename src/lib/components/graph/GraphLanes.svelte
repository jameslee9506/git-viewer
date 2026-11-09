<!--
  Lanes SVG for the rendered window of the graph list. Lives in the VirtualList overlay, one row
  above row `start` and right of the branch / tag column (`left`); about
  ten batched paths plus one node per row: the author's identicon in a lane-colored ring for a commit
  (util/identicon.ts), a hollow dashed ring for the WIP row, a hollow square for a stash.
  Colors come from CSS classes and custom properties only (WebKit ignores var() in SVG presentation
  attributes).
-->
<script lang="ts">
  import { getRepo } from '../../state/repo.svelte';
  import { AVATAR_R, identicon } from '../../util/identicon';
  import { buildLanes, MAX_LANES, naturalWidth, RING_W, ROW_H } from './geometry';

  interface Props {
    /** First rendered row. */
    start: number;
    /** End of the rendered rows (exclusive). */
    end: number;
    /** Graph column width in px. */
    width: number;
    /** Graph column's left edge in px: the branch / tag column's width (0 without it). */
    left?: number;
  }

  let { start, end, width, left = 0 }: Props = $props();

  const graph = getRepo().graph;
  const uid = $props.id();
  /** The avatar clip's id: url(#…) resolves document-wide and inactive tabs keep their graph mounted. */
  const clip = `avclip-${uid}`;

  /** Half the side of a stash's square: about the mass of the avatar disc and its ring. */
  const STASH_H = 7.5;

  const model = $derived.by(() => {
    const total = graph.total;
    return buildLanes((i) => graph.row(i), start, Math.min(end, total), total, width);
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
    style:left="{left}px"
    aria-hidden="true"
  >
    <defs>
      <clipPath id={clip}><circle r={AVATAR_R} /></clipPath>
    </defs>
    {#each model.paths as p (p.cls)}
      <path class={p.cls} d={p.d} />
    {/each}
    {#each model.nodes as n (n.row)}
      {@const on = n.row === sel}
      {@const em = on || n.row === cmp}
      <!-- The selected / compare row's node grows by half a pixel; a ring keeps its inner edge on the disc. -->
      {@const g = em ? 0.5 : 0}
      {#if n.kind === 'stash'}
        <rect
          class="h c{n.color}"
          x={n.x - STASH_H - g}
          y={n.y - STASH_H - g}
          width={2 * (STASH_H + g)}
          height={2 * (STASH_H + g)}
          rx="2"
        />
      {:else if n.kind === 'wip'}
        <circle class="h d wip" cx={n.x} cy={n.y} r={AVATAR_R + RING_W / 2 + g} />
      {:else}
        {@const id = identicon(n.author, n.email)}
        <g class="av" transform="translate({n.x} {n.y})" style:--c="var(--lane-{id.hue})">
          <circle class="bg" r={AVATAR_R} />
          <path class="px" d={id.path} clip-path="url(#{clip})" />
          <circle class="ring c{n.color}" class:em r={AVATAR_R + RING_W / 2 + g / 2} />
        </g>
      {/if}
    {/each}
  </svg>
{/if}

<style>
  .graph {
    position: absolute;
    z-index: 1;
    overflow: hidden;
    pointer-events: none;
  }
  .graph.fade {
    -webkit-mask-image: linear-gradient(to right, var(--bg-graph) calc(100% - 12px), transparent);
    mask-image: linear-gradient(to right, var(--bg-graph) calc(100% - 12px), transparent);
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
  /* avatar: the glyph in the author's hue (--c) on an opaque tint of it — the lane line runs under the
     disc — inside the lane-colored ring, which the lines meet */
  .av .bg {
    fill: color-mix(in srgb, var(--c) 25%, var(--bg-graph));
  }
  .av .px {
    fill: var(--c);
    stroke: none;
    shape-rendering: crispEdges;
  }
  .av .ring {
    fill: none;
    stroke: currentColor;
    stroke-width: 1.5;
  }
  .av .ring.em {
    stroke-width: 2;
  }
  /* hollow nodes: graph background fill (the row's highlight band starts at the node's center, so the
     hole stays graph-colored in every row state), lane-colored outline */
  .h {
    fill: var(--bg-graph);
    stroke: currentColor;
    stroke-width: 1.5;
  }
  .h.wip {
    stroke-dasharray: 2 2;
  }
</style>
