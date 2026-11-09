<!--
  The fixed-height head of the commit and stash views: the header bar, the message box and the meta
  lines (author / committer, stashed time). Its height is `settings.detailsHeadHeight`, the same for
  every commit and stash so the files below always start at the same place; the message box takes
  what the bar and the meta lines leave and scrolls inside. Its bottom edge is a splitter that sets
  the height (double-click: the default). The head only leaves the preference to keep one subject
  line in view (a wrapped bar, a committer line) or to leave the file list `RESERVE`.
-->
<script lang="ts">
  import type { Snippet } from 'svelte';
  import { DEFAULT_SETTINGS, settings } from '../../state/settings.svelte';
  import Resizer from '../shell/Resizer.svelte';

  interface Props {
    /** Height of the view in px (0 before it is measured): bounds the head. */
    panelHeight: number;
    bar: Snippet;
    /** The message box's text (subject, body). */
    message: Snippet;
    meta?: Snippet;
  }

  /** Space above the message box, and the splitter under it. */
  const MSG_TOP = 12;
  const GRIP_H = 1;
  /** Smallest message box: one subject line (15px × 1.4) with its padding and top border. */
  const MSG_MIN = 42;
  /** Room kept below the head: the change summary, the file toolbar and three file rows. */
  const RESERVE = 160;

  let { panelHeight, bar, message, meta }: Props = $props();

  let barH = $state(0);
  let metaH = $state(0);

  const min = $derived(barH + MSG_TOP + MSG_MIN + GRIP_H + metaH);
  const max = $derived(Math.max(min, panelHeight > 0 ? panelHeight - RESERVE : settings.detailsHeadHeight));
  const height = $derived(Math.min(max, Math.max(min, settings.detailsHeadHeight)));
</script>

<div class="head" style:height="{height}px">
  <div class="bar" bind:clientHeight={barH}>{@render bar()}</div>
  <div class="msg">
    <div class="text">{@render message()}</div>
  </div>
  <div class="grip">
    <Resizer
      bind:value={() => height, (v) => (settings.detailsHeadHeight = v)}
      {min}
      {max}
      defaultValue={DEFAULT_SETTINGS.detailsHeadHeight}
      side="top"
    />
  </div>
  <div class="meta" bind:clientHeight={metaH}>{@render meta?.()}</div>
</div>

<style>
  .head {
    flex: none;
    display: flex;
    flex-direction: column;
    min-width: 0;
    overflow: hidden;
  }
  .bar,
  .meta {
    flex: none;
  }
  /* Paper on the band, square; the splitter under it is its bottom edge. */
  .msg {
    flex: 1 1 0;
    min-height: 0;
    margin: 12px 12px 0;
    overflow-x: hidden;
    overflow-y: auto;
    background: var(--bg-inset);
    border: 1px solid var(--border);
    border-bottom: 0;
  }
  /* The padding on an inner box: a scroller's own bottom padding is not part of its scroll range. */
  .text {
    padding: 10px 8px;
  }
  .grip {
    flex: none;
    margin: 0 12px;
  }
</style>
