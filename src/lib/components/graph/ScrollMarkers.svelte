<!--
  Overview ticks over the graph list's scrollbar track: search hits, HEAD row, selected row.
  One path for all hits (deduplicated per pixel), so thousands of matches stay cheap.
-->
<script lang="ts">
  import { getRepo } from '../../state/repo.svelte';

  interface Props {
    /** Number of graph rows the track represents. */
    total: number;
  }

  let { total }: Props = $props();

  const graph = getRepo().graph;
  const MAX_MARKS = 2000;
  const TICK = 2;

  let height = $state(0);

  function tickY(row: number): number {
    return Math.max(0, Math.min(height - TICK, Math.floor((row / total) * height)));
  }

  const hits = $derived.by(() => {
    // Matches of the current generation only: older rows would point at other commits.
    const rows = graph.matchRows;
    if (rows.length === 0 || total <= 0 || height <= 0) return '';
    let d = '';
    let last = -1;
    let n = 0;
    for (const r of rows) {
      if (r >= total) break;
      const y = tickY(r);
      if (y === last) continue;
      last = y;
      d += `M0 ${y}h6v${TICK}h-6z`;
      if (++n >= MAX_MARKS) break;
    }
    return d;
  });
  const head = $derived(graph.summary?.headRow ?? null);
  const sel = $derived(graph.selectedRow);
</script>

<div class="markers" bind:clientHeight={height} aria-hidden="true">
  {#if total > 0 && height > 0}
    <svg width="6" {height}>
      {#if hits}
        <path class="hit" d={hits} />
      {/if}
      {#if head != null && head < total}
        <rect class="head" x="0" y={tickY(head)} width="6" height={TICK} />
      {/if}
      {#if sel != null && sel < total}
        <rect class="sel" x="0" y={tickY(sel)} width="6" height={TICK} />
      {/if}
    </svg>
  {/if}
</div>

<style>
  .markers {
    position: absolute;
    top: 0;
    bottom: 0;
    right: 2px;
    width: 6px;
    z-index: 2;
    pointer-events: none;
  }
  svg {
    display: block;
  }
  .hit {
    fill: var(--mark);
  }
  .head {
    fill: var(--badge-head-bg);
  }
  .sel {
    fill: var(--text-0);
  }
</style>
