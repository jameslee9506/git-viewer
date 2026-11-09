<!--
  Unified diff: one VirtualList (20px rows) with native horizontal scrolling. Row = [old no][new no][code];
  the line-number gutter is sticky with an opaque background and the background carries the change type.
  Exported: step(dir) jumps to the next/previous change block, focus().
-->
<script lang="ts">
  import { untrack } from 'svelte';
  import type { Token } from '../../highlight';
  import VirtualList from '../common/VirtualList.svelte';
  import DiffLineText from './DiffLineText.svelte';
  import { intraLookup } from './intraline';
  import {
    CRMARK,
    NOEOL,
    blockNavKey,
    findBlock,
    isChangeRow,
    recallTop,
    rememberTop,
    rowOfTopPos,
    topPosOf,
    type DLine,
    type DiffModel,
  } from './model';

  interface Props {
    model: DiffModel;
    oldTokens: Token[][] | null;
    newTokens: Token[][] | null;
    /** Monospace character advance in px. */
    charWidth: number;
    /** Diff item identity: the scroll position is remembered per key (shared by both views). */
    scrollKey: string;
    /** Request identity: when it changes the remembered position is restored (refetches keep it). */
    viewKey: string;
  }

  let { model, oldTokens, newTokens, charWidth, scrollKey, viewKey }: Props = $props();

  const ROW = 20;
  /** Rows of context kept above a change block when jumping to it. */
  const LEAD = 2;

  let list: VirtualList | undefined = $state();
  /** Current change block of n/p navigation (marked in the gutter). */
  let nav = $state.raw<{ model: DiffModel; block: number; from: number; to: number; top: number } | null>(null);

  const intra = $derived(intraLookup(model));
  const lnW = $derived(Math.ceil(Math.max(3, String(model.maxNo).length) * charWidth) + 14);
  const contentWidth = $derived(2 * lnW + Math.ceil(model.maxCols * charWidth) + 40);
  const marked = $derived(nav && nav.model === model ? nav : null);

  let restoredKey: string | null = null;

  // A different diff: restore its remembered position (or the top), reset the horizontal offset.
  $effect(() => {
    const key = viewKey;
    const m = model;
    const l = list;
    if (!l || key === restoredKey) return;
    restoredKey = key;
    untrack(() => {
      const pos = recallTop(scrollKey);
      l.scrollToIndex(pos ? rowOfTopPos(m, 'unified', pos) : 0, 'start');
      const vp = l.getViewport();
      if (vp) vp.scrollLeft = 0;
    });
  });

  function onscroll(top: number) {
    const pos = topPosOf(model, 'unified', Math.floor(top / ROW));
    if (pos) rememberTop(scrollKey, pos);
  }

  function tokensOf(ln: DLine): Token[] | null {
    if (ln.kind === 2) return oldTokens?.[ln.oi] ?? null;
    return newTokens?.[ln.ni] ?? (ln.kind === 0 ? oldTokens?.[ln.oi] : null) ?? null;
  }

  /** Jump to the next (1) / previous (-1) change block, from the row at 1/3 of the viewport. */
  export function step(dir: 1 | -1): void {
    const vp = list?.getViewport();
    const blocks = model.uniBlocks;
    if (!list || !vp || blocks.length === 0) return;
    const top = list.getScrollTop();
    const b =
      nav && nav.model === model && Math.abs(nav.top - top) < 2
        ? nav.block + dir
        : findBlock(blocks, Math.floor((top + vp.clientHeight / 3) / ROW), dir);
    if (b < 0 || b >= blocks.length) return;
    const from = blocks[b];
    let to = from + 1;
    while (to < model.unified.length && isChangeRow(model, 'unified', to)) to++;
    list.scrollToIndex(Math.max(0, from - LEAD), 'start');
    nav = { model, block: b, from, to, top: list.getScrollTop() };
  }

  export function focus(): void {
    list?.focus();
  }

  function onkeydown(e: KeyboardEvent) {
    const dir = blockNavKey(e);
    if (!dir) return;
    e.preventDefault();
    step(dir);
  }
</script>

<div class="uni" style:--ln-w="{lnW}px">
  <VirtualList
    bind:this={list}
    count={model.unified.length}
    rowHeight={ROW}
    overscan={12}
    horizontal
    {contentWidth}
    tabindex={0}
    {onscroll}
    {onkeydown}
  >
    {#snippet row(i)}
      {@const r = model.unified[i]}
      {#if r.t === 'hunk'}
        <div class="row hunk" title={r.hidden ? `${r.hidden} lines` : undefined}>
          <span class="gutter"></span>
          <span class="heading">{r.heading}</span>
        </div>
      {:else}
        {@const ln = model.lines[r.line]}
        <div class="row k{ln.kind}" class:nav={marked !== null && i >= marked.from && i < marked.to}>
          <span class="gutter"><span class="ln" data-n={ln.old}></span><span class="ln" data-n={ln.new}></span></span>
          <span class="code selectable"><DiffLineText
              text={ln.text}
              tokens={tokensOf(ln)}
              ranges={ln.kind === 0 ? null : intra(r.line)}
              kind={ln.kind}
            />{#if ln.flags & CRMARK}<span class="mark cr"></span>{/if}{#if ln.flags & NOEOL}<span
                class="mark noeol"
              ></span>{/if}</span>
        </div>
      {/if}
    {/snippet}
  </VirtualList>
</div>

<style>
  .uni {
    height: 100%;
    font-family: var(--font-mono);
    font-size: var(--fs-code);
    line-height: var(--line-h);
    font-variant-ligatures: none;
    tab-size: 4;
  }
  .row {
    display: flex;
    height: 100%;
  }
  .gutter {
    position: sticky;
    left: 0;
    z-index: 1;
    display: flex;
    flex: none;
    background: var(--bg-1);
    border-right: 1px solid var(--border);
  }
  .ln {
    width: var(--ln-w);
    padding-right: 8px;
    text-align: right;
    color: var(--text-2);
    user-select: none;
    -webkit-user-select: none;
  }
  .ln::before {
    content: attr(data-n);
  }
  .code {
    flex: 1 0 auto;
    padding: 0 24px 0 8px;
    white-space: pre;
  }
  .k1 {
    background: var(--add-bg);
  }
  .k2 {
    background: var(--del-bg);
  }
  .k1 .gutter {
    background: linear-gradient(var(--add-gutter), var(--add-gutter)), var(--bg-1);
  }
  .k2 .gutter {
    background: linear-gradient(var(--del-gutter), var(--del-gutter)), var(--bg-1);
  }
  .nav .gutter {
    box-shadow: inset 2px 0 0 var(--accent);
  }
  .hunk {
    background: var(--hunk-bg);
    color: var(--hunk-fg);
  }
  .hunk .gutter {
    width: calc(2 * var(--ln-w) + 1px);
    background: linear-gradient(var(--hunk-bg), var(--hunk-bg)), var(--bg-1);
  }
  .heading {
    position: sticky;
    left: calc(2 * var(--ln-w) + 1px);
    padding-left: 8px;
    white-space: pre;
  }
  .mark {
    color: var(--text-2);
    user-select: none;
    -webkit-user-select: none;
  }
  .cr::before {
    content: '␍';
  }
  .noeol::before {
    content: '∅';
    padding-left: 4px;
  }
</style>
