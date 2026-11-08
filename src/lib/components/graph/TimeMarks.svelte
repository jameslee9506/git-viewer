<!--
  Time marks: the rendered rows' dates as pills over the rows' right edge, one wherever a row's date
  differs from the row above (relative dates as the date column shows them, absolute ones by day).
  A mark sits on the boundary above its row and reads as a divider: the rows below it are that old
  until the next mark. Rendered in the VirtualList overlay while the date column gives way to the
  subject (geometry.ts layoutColumns). A mark carries its row's data-row, so clicking it picks the row.
-->
<script lang="ts">
  import { getRepo } from '../../state/repo.svelte';
  import { settings } from '../../state/settings.svelte';
  import { absoluteDate, absoluteTime, relativeTime } from '../../util/format';
  import { ROW_H } from './geometry';

  interface Props {
    /** First rendered row. */
    start: number;
    /** End of the rendered rows (exclusive). */
    end: number;
    /** Bumped once a minute while dates are relative, so a "5m" mark ages in rows that stay rendered. */
    tick?: number;
  }

  let { start, end, tick = 0 }: Props = $props();

  const graph = getRepo().graph;

  /** Mark height in px; it straddles the row boundary by half of it each way. */
  const MARK_H = 16;

  interface Mark {
    row: number;
    /** Top edge in px from the overlay's top (row `start`). */
    top: number;
    text: string;
    title: string;
  }

  const marks = $derived.by(() => {
    void tick;
    const rel = settings.dateFormat === 'relative';
    const text = (t: number) => (t ? (rel ? relativeTime(t) : absoluteDate(t)) : '');
    const out: Mark[] = [];
    const last = Math.min(end, graph.total);
    let prev = start > 0 ? graph.row(start - 1) : undefined;
    for (let i = start; i < last; i++) {
      const r = graph.row(i);
      if (r) {
        const s = text(r.time);
        // A boundary: the list's top, a row above that is not loaded yet, or another date above.
        if (s && (i === 0 || !prev || text(prev.time) !== s)) {
          out.push({
            row: i,
            top: Math.max(0, (i - start) * ROW_H - MARK_H / 2),
            text: s,
            title: absoluteTime(r.time),
          });
        }
      }
      prev = r;
    }
    return out;
  });
</script>

{#each marks as m (m.row)}
  <span class="mark" style:top="{m.top}px" title={m.title} data-row={m.row}>{m.text}</span>
{/each}

<style>
  .mark {
    position: absolute;
    right: 4px;
    z-index: 2;
    display: inline-flex;
    align-items: center;
    height: 16px;
    padding: 0 5px;
    border: 1px solid var(--border-strong);
    border-radius: var(--radius-sm);
    background: var(--bg-float);
    color: var(--text-1);
    font-size: var(--fs-xs);
    line-height: 1;
    white-space: nowrap;
    font-variant-numeric: tabular-nums;
  }
</style>
