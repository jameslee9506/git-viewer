<!--
  Side-by-side diff: one vertical VirtualList; row = [old no][old code] | [new no][new code], 50% each.
  Both code columns share one horizontal offset (--sx) driven by wheel deltaX / shift+wheel and mirrored
  by a thin native bar at the bottom. Text selection is locked to the side where it started.
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
  const LEAD = 2;
  /** Code cell padding: left (inside the cell) and room kept after the longest line. */
  const PAD_L = 8;
  const PAD_R = 24;
  /** Width taken by the list's vertical scrollbar (base.css). */
  const SCROLLBAR = 10;
  /** Horizontal offset change per ← / → press (the native arrow-key scroll step). */
  const H_STEP = 40;

  let root: HTMLDivElement | undefined = $state();
  let bar: HTMLDivElement | undefined = $state();
  let list: VirtualList | undefined = $state();
  let width = $state(0);
  let sx = $state(0);
  /** Side where the current text selection started (the other side is not selectable). */
  let side = $state<'old' | 'new' | null>(null);
  let nav = $state.raw<{ model: DiffModel; block: number; from: number; to: number; top: number } | null>(null);

  const intra = $derived(intraLookup(model));
  const lnW = $derived(Math.ceil(Math.max(3, String(model.maxNo).length) * charWidth) + 14);
  const codeW = $derived(Math.max(0, (width - SCROLLBAR) / 2 - lnW - PAD_L - 1));
  const maxSx = $derived(Math.max(0, Math.ceil(model.maxCols * charWidth) + PAD_R - codeW));
  const x = $derived(Math.min(Math.max(0, sx), maxSx));
  const marked = $derived(nav && nav.model === model ? nav : null);

  let restoredKey: string | null = null;

  $effect(() => {
    const key = viewKey;
    const m = model;
    const l = list;
    if (!l || key === restoredKey) return;
    restoredKey = key;
    untrack(() => {
      const pos = recallTop(scrollKey);
      l.scrollToIndex(pos ? rowOfTopPos(m, 'split', pos) : 0, 'start');
      sx = 0;
    });
  });

  // Mirror the offset into the bottom bar (its own scroll events write back to sx).
  $effect(() => {
    const b = bar;
    const v = x;
    if (b && Math.abs(b.scrollLeft - v) > 0.5) b.scrollLeft = v;
  });

  // Non-passive wheel: horizontal deltas move the shared offset; vertical ones scroll natively.
  $effect(() => {
    const el = root;
    if (!el) return;
    const onwheel = (e: WheelEvent) => {
      const unit = e.deltaMode === 1 ? 16 : e.deltaMode === 2 ? el.clientWidth : 1;
      let dx = e.deltaX * unit;
      const dy = e.deltaY * unit;
      if (!dx && e.shiftKey) dx = dy;
      else if (Math.abs(dx) <= Math.abs(dy)) return;
      if (!dx || maxSx <= 0) return;
      e.preventDefault();
      sx = Math.min(maxSx, Math.max(0, x + dx));
    };
    const onmousedown = (e: MouseEvent) => {
      if (e.button !== 0) return;
      const half = (e.target as Element | null)?.closest?.('.half');
      side = half ? (half.classList.contains('new') ? 'new' : 'old') : null;
    };
    el.addEventListener('wheel', onwheel, { passive: false });
    el.addEventListener('mousedown', onmousedown);
    return () => {
      el.removeEventListener('wheel', onwheel);
      el.removeEventListener('mousedown', onmousedown);
    };
  });

  function onscroll(top: number) {
    const pos = topPosOf(model, 'split', Math.floor(top / ROW));
    if (pos) rememberTop(scrollKey, pos);
  }

  function onbar() {
    if (bar) sx = bar.scrollLeft;
  }

  function tokensOf(ln: DLine, s: 'old' | 'new'): Token[] | null {
    if (s === 'old') return oldTokens?.[ln.oi] ?? (ln.kind === 0 ? newTokens?.[ln.ni] : null) ?? null;
    return newTokens?.[ln.ni] ?? (ln.kind === 0 ? oldTokens?.[ln.oi] : null) ?? null;
  }

  /** Jump to the next (1) / previous (-1) change block, from the row at 1/3 of the viewport. */
  export function step(dir: 1 | -1): void {
    const vp = list?.getViewport();
    const blocks = model.splitBlocks;
    if (!list || !vp || blocks.length === 0) return;
    const top = list.getScrollTop();
    const b =
      nav && nav.model === model && Math.abs(nav.top - top) < 2
        ? nav.block + dir
        : findBlock(blocks, Math.floor((top + vp.clientHeight / 3) / ROW), dir);
    if (b < 0 || b >= blocks.length) return;
    const from = blocks[b];
    let to = from + 1;
    while (to < model.split.length && isChangeRow(model, 'split', to)) to++;
    list.scrollToIndex(Math.max(0, from - LEAD), 'start');
    nav = { model, block: b, from, to, top: list.getScrollTop() };
  }

  export function focus(): void {
    list?.focus();
  }

  function onkeydown(e: KeyboardEvent) {
    const dir = blockNavKey(e);
    if (dir) {
      e.preventDefault();
      step(dir);
      return;
    }
    // ← → move the shared horizontal offset (the list itself only scrolls vertically).
    const h = e.code === 'ArrowRight' ? 1 : e.code === 'ArrowLeft' ? -1 : 0;
    if (!h || e.altKey || e.metaKey || e.ctrlKey || e.shiftKey || e.isComposing || maxSx <= 0) return;
    e.preventDefault();
    sx = Math.min(maxSx, Math.max(0, x + h * H_STEP));
  }
</script>

<div class="split" bind:this={root} bind:clientWidth={width} style:--ln-w="{lnW}px" style:--sx={x} data-sel={side}>
  <div class="body">
    <VirtualList bind:this={list} count={model.split.length} rowHeight={ROW} overscan={12} tabindex={0} {onscroll} {onkeydown}>
      {#snippet row(i)}
        {@const r = model.split[i]}
        {#if r.t === 'hunk'}
          <div class="row hunk" title={r.hidden ? `${r.hidden} lines` : undefined}>
            <span class="ln"></span>
            <span class="heading">{r.heading}</span>
          </div>
        {:else}
          <div class="row" class:nav={marked !== null && i >= marked.from && i < marked.to}>
            {@render half(r.left, 'old')}
            {@render half(r.right, 'new')}
          </div>
        {/if}
      {/snippet}
    </VirtualList>
  </div>
  {#if maxSx > 0}
    <div class="hbar" bind:this={bar} onscroll={onbar}><div style:width="calc(100% + {maxSx}px)"></div></div>
  {/if}
</div>

{#snippet half(li: number, s: 'old' | 'new')}
  {#if li < 0}
    <div class="half fill {s}"><span class="ln"></span><span class="code"></span></div>
  {:else}
    {@const ln = model.lines[li]}
    <div class="half {s} k{ln.kind}">
      <span class="ln" data-n={s === 'old' ? ln.old : ln.new}></span>
      <span class="code"><span class="sx"><DiffLineText
            text={ln.text}
            tokens={tokensOf(ln, s)}
            ranges={ln.kind === 0 ? null : intra(li)}
            kind={ln.kind}
          />{#if ln.flags & CRMARK}<span class="mark cr"></span>{/if}{#if ln.flags & NOEOL}<span
              class="mark noeol"
            ></span>{/if}</span></span>
    </div>
  {/if}
{/snippet}

<style>
  .split {
    display: flex;
    flex-direction: column;
    height: 100%;
    font-family: var(--font-mono);
    font-size: var(--fs-code);
    line-height: var(--line-h);
    font-variant-ligatures: none;
    tab-size: 4;
  }
  .body {
    flex: 1 1 auto;
    min-height: 0;
  }
  .row {
    display: flex;
    height: 100%;
  }
  .half {
    display: flex;
    flex: 1 1 0;
    min-width: 0;
  }
  .half.old {
    border-right: 1px solid var(--border);
  }
  .ln {
    flex: none;
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
    flex: 1 1 auto;
    min-width: 0;
    overflow: hidden;
    padding-left: 8px;
    white-space: pre;
    user-select: text;
    -webkit-user-select: text;
    cursor: text;
  }
  .split[data-sel='old'] .new .code,
  .split[data-sel='new'] .old .code {
    user-select: none;
    -webkit-user-select: none;
  }
  .sx {
    display: inline-block;
    transform: translateX(calc(var(--sx) * -1px));
  }
  .k1 {
    background: var(--add-bg);
  }
  .k2 {
    background: var(--del-bg);
  }
  .k1 .ln {
    background: linear-gradient(var(--add-gutter), var(--add-gutter)), var(--bg-1);
  }
  .k2 .ln {
    background: linear-gradient(var(--del-gutter), var(--del-gutter)), var(--bg-1);
  }
  .fill {
    background: var(--filler-bg);
  }
  .nav .old .ln {
    box-shadow: inset 2px 0 0 var(--accent);
  }
  .hunk {
    background: var(--hunk-bg);
    color: var(--hunk-fg);
  }
  .heading {
    flex: 1 1 auto;
    min-width: 0;
    padding-left: 8px;
    overflow: hidden;
    white-space: pre;
    text-overflow: ellipsis;
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
  .hbar {
    flex: none;
    height: 10px;
    overflow-x: auto;
    overflow-y: hidden;
  }
  .hbar > div {
    height: 1px;
  }
</style>
