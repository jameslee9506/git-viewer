<!--
  Read-only file content for a BlobSpec: highlighted, virtualized text with a line-number gutter;
  images on a checkerboard with `W×H · size`; binary / truncated / empty placeholders. The previous
  content stays visible until the next one is ready, and stale responses are dropped.
  The module script holds small helpers shared by the other file views.
-->
<script module lang="ts">
  import { highlightLines, languageForPath, type Token } from '../../highlight';
  import { app } from '../../state/app.svelte';
  import type { RepoStore } from '../../state/repo.svelte';
  import { contextMenu } from '../../state/ui.svelte';
  import type { BlobSpec } from '../../types';
  import { isComposing, isEditable } from '../../util/keys';

  /** Code line height in px (matches --line-h). */
  export const LINE_H = 20;

  const endsWithCR = (l: string) => l.charCodeAt(l.length - 1) === 13;

  /** Lines without a trailing CR (the same array when no line has one). */
  export function stripCR(lines: string[]): string[] {
    return lines.some(endsWithCR) ? lines.map((l) => (endsWithCR(l) ? l.slice(0, -1) : l)) : lines;
  }

  /** File text → display lines: no phantom line after the final newline, trailing CRs stripped. */
  export function splitLines(text: string): string[] {
    if (text === '') return [];
    const lines = text.split('\n');
    if (lines[lines.length - 1] === '') lines.pop();
    return stripCR(lines);
  }

  export function sameLines(a: string[], b: string[]): boolean {
    if (a === b) return true;
    if (a.length !== b.length) return false;
    for (let i = 0; i < a.length; i++) if (a[i] !== b[i]) return false;
    return true;
  }

  const isWide = (c: number) =>
    (c >= 0x1100 && c <= 0x115f) ||
    (c >= 0x2e80 && c <= 0xa4cf) ||
    (c >= 0xac00 && c <= 0xd7a3) ||
    (c >= 0xf900 && c <= 0xfaff) ||
    (c >= 0xfe30 && c <= 0xfe4f) ||
    (c >= 0xff00 && c <= 0xff60) ||
    (c >= 0xffe0 && c <= 0xffe6);

  /** Width of the longest line in monospace columns (tab stops of 4; CJK and astral chars = 2). */
  export function maxColumns(lines: string[]): number {
    let max = 0;
    for (const l of lines) {
      // A UTF-16 unit never spans more than 4 columns: skip lines that cannot beat the max.
      if (l.length * 4 <= max) continue;
      let col = 0;
      for (let i = 0; i < l.length; i++) {
        const c = l.charCodeAt(i);
        if (c === 9) col += 4 - (col % 4);
        else if (c >= 0xd800 && c <= 0xdbff) {
          col += 2;
          i++;
        } else col += isWide(c) ? 2 : 1;
      }
      if (col > max) max = col;
    }
    return max;
  }

  /** `<full oid>^` → `<7 hex>^`; any other rev unchanged. */
  export function shortRev(rev: string): string {
    const m = /^([0-9a-f]{40,64})(.*)$/i.exec(rev);
    return m ? m[1].slice(0, 7) + m[2] : rev;
  }

  /** Revs pinned to an object id (`<oid>`, `<oid>^`, `<oid>~2`): their results never change. */
  export function isOidRev(rev: string): boolean {
    return /^[0-9a-f]{40,64}(?:[~^]\d*)*$/i.test(rev);
  }

  /** Tokens for `lines` in the language of `path`; null when unsupported or failed. */
  export function highlightFor(path: string, lines: string[]): Promise<Token[][] | null> {
    const lang = languageForPath(path);
    if (!lang || lines.length === 0) return Promise.resolve(null);
    return highlightLines(lang, lines).catch(() => null);
  }

  export const PENDING = Symbol('pending');

  /** The value of `p` when it settles within `ms`, else PENDING (`p` keeps running). */
  export function within<T>(p: Promise<T>, ms: number): Promise<T | typeof PENDING> {
    let timer: ReturnType<typeof setTimeout> | undefined;
    const late = new Promise<typeof PENDING>((resolve) => (timer = setTimeout(() => resolve(PENDING), ms)));
    return Promise.race([p, late]).finally(() => clearTimeout(timer));
  }

  interface Point {
    line: number;
    col: number;
  }

  /** Map a selection boundary to (line, column) in `[data-line]` rows with a `[data-code]` cell. */
  function locate(root: HTMLElement, lines: string[], node: Node, offset: number, end: boolean): Point | null {
    const el = node instanceof Element ? node : node.parentElement;
    const row = el?.closest<HTMLElement>('[data-line]');
    if (!row) {
      // Boundary between children of a container (e.g. a list row wrapper): snap to the nearest row.
      if (!(node instanceof Element) || !root.contains(node)) return null;
      const probe = node.childNodes[end ? offset - 1 : offset];
      if (!(probe instanceof Element)) return null;
      const found = probe.matches('[data-line]') ? [probe] : Array.from(probe.querySelectorAll('[data-line]'));
      const hit = (end ? found[found.length - 1] : found[0]) as HTMLElement | undefined;
      if (!hit) return null;
      const line = Number(hit.dataset.line);
      const text = lines[line];
      return text === undefined ? null : { line, col: end ? text.length : 0 };
    }
    if (!root.contains(row)) return null;
    const line = Number(row.dataset.line);
    const text = lines[line];
    if (text === undefined) return null;
    const code = row.querySelector('[data-code]');
    if (!code) return { line, col: 0 };
    const range = document.createRange();
    range.selectNodeContents(code);
    const cmp = range.comparePoint(node, offset);
    if (cmp !== 0) return { line, col: cmp < 0 ? 0 : text.length };
    range.setEnd(node, offset);
    return { line, col: Math.min(text.length, range.toString().length) };
  }

  /**
   * Text of the current selection when both ends lie in code rows under `root`, rebuilt from
   * `lines` (gutters and line numbers are never copied). Null → let the browser copy.
   */
  export function selectedText(root: HTMLElement, lines: string[]): string | null {
    const sel = document.getSelection();
    if (!sel || sel.isCollapsed || sel.rangeCount === 0) return null;
    const r = sel.getRangeAt(0);
    let a: Point | null;
    let b: Point | null;
    try {
      a = locate(root, lines, r.startContainer, r.startOffset, false);
      b = locate(root, lines, r.endContainer, r.endOffset, true);
    } catch {
      // Range APIs throw for boundaries they cannot compare: leave the copy to the browser.
      return null;
    }
    if (!a || !b || b.line < a.line) return null;
    if (a.line === b.line) return lines[a.line].slice(a.col, b.col);
    const out = [lines[a.line].slice(a.col)];
    for (let i = a.line + 1; i < b.line; i++) out.push(lines[i]);
    out.push(lines[b.line].slice(0, b.col));
    return out.join('\n');
  }

  /**
   * Esc in the active tab (outside text fields, with no context menu open and not handled by an
   * inner owner) returns one viewer level, the graph last (`repo.back()`); a held Esc steps only
   * once. Call from onMount; returns the cleanup.
   */
  export function escapeBack(repo: RepoStore): () => void {
    let menuWasOpen = false;
    // Capture phase: the context menu may already have closed itself by the time we see the event.
    const capture = (e: KeyboardEvent) => {
      if (e.code === 'Escape') menuWasOpen = contextMenu.open;
    };
    const bubble = (e: KeyboardEvent) => {
      if (e.code !== 'Escape' || menuWasOpen || e.defaultPrevented || isComposing(e)) return;
      if (app.active?.repo !== repo || isEditable(e.target)) return;
      // Auto-repeat is consumed without stepping (App's last-layer fallback would take it otherwise).
      e.preventDefault();
      if (!e.repeat) repo.back();
    };
    window.addEventListener('keydown', capture, true);
    window.addEventListener('keydown', bubble);
    return () => {
      window.removeEventListener('keydown', capture, true);
      window.removeEventListener('keydown', bubble);
    };
  }

  function specKey(s: BlobSpec): string {
    switch (s.kind) {
      case 'rev':
        return `rev\0${s.rev}\0${s.path}`;
      case 'index':
        return `index\0${s.cwd}\0${s.path}`;
      case 'worktree':
        return `worktree\0${s.cwd}\0${s.path}`;
      case 'oid':
        return `oid\0${s.oid}`;
    }
  }

  const MIME: Record<string, string> = {
    png: 'image/png',
    jpg: 'image/jpeg',
    jpeg: 'image/jpeg',
    gif: 'image/gif',
    webp: 'image/webp',
    bmp: 'image/bmp',
    ico: 'image/x-icon',
    avif: 'image/avif',
    tif: 'image/tiff',
    tiff: 'image/tiff',
    heic: 'image/heic',
  };
</script>

<script lang="ts">
  import ArrowDownToLine from '@lucide/svelte/icons/arrow-down-to-line';
  import FileImage from '@lucide/svelte/icons/file-image';
  import FileQuestionMark from '@lucide/svelte/icons/file-question-mark';
  import FileText from '@lucide/svelte/icons/file-text';
  import FileX from '@lucide/svelte/icons/file-x';
  import { tick, untrack } from 'svelte';
  import { api, errorMessage } from '../../api';
  import { getRepo } from '../../state/repo.svelte';
  import { basename, extname, formatBytes, isImagePath } from '../../util/format';
  import VirtualList from '../common/VirtualList.svelte';
  import DiffLineText from '../diff/DiffLineText.svelte';

  interface Props {
    spec: BlobSpec;
    /** Repo-relative path: picks the language and image handling (also for `oid` specs). */
    path: string;
  }

  let { spec, path }: Props = $props();

  type Content =
    | { kind: 'text'; key: string; path: string; lines: string[]; cols: number }
    | { kind: 'image'; key: string; url: string; size: number }
    | { kind: 'binary'; key: string; size: number }
    | { kind: 'truncated'; key: string; size: number; image: boolean }
    | { kind: 'empty'; key: string }
    | { kind: 'error'; key: string; message: string };

  /** Cap for the explicit "load anyway" of a blob over the default 16 MiB. */
  const FULL_BYTES = 64 * 1024 * 1024;
  /** How long ready text waits for its highlighting before showing plain (avoids a flash). */
  const HL_WAIT = 100;

  const repo = getRepo();
  let content = $state.raw<Content | null>(null);
  let tokens = $state.raw<Token[][] | null>(null);
  let loading = $state(false);
  /** `W×H` of the shown image once decoded. */
  let dims = $state<string | null>(null);
  /** Key the user asked to load past the default size cap. */
  let fullKey = $state<string | null>(null);
  let root: HTMLDivElement | undefined = $state();
  let list: VirtualList | undefined = $state();
  let seq = 0;
  /** focus() asked for before the text was ready: which content, and where focus was at that time. */
  let focusReq: { key: string; from: Element | null } | null = null;

  const key = $derived(`${specKey(spec)}\0${path}`);
  const live = $derived(spec.kind === 'worktree' || spec.kind === 'index');
  const stale = $derived(loading && content != null && content.key !== key);
  const digits = $derived(content?.kind === 'text' ? Math.max(3, String(content.lines.length).length) : 3);

  $effect(() => {
    const k = key;
    const max = fullKey === k ? FULL_BYTES : null;
    // Working-tree and index content can change under the same spec.
    if (live) void repo.worktreeVersion;
    untrack(() => void load(k, spec, path, max));
  });

  async function load(k: string, s: BlobSpec, p: string, max: number | null): Promise<void> {
    const id = ++seq;
    loading = true;
    const image = isImagePath(p) && extname(p) !== 'svg';
    let next: Content;
    try {
      const blob = await api.readBlob(repo.id, s, image ? 'base64' : 'text', max);
      if (id !== seq) return;
      if (blob.truncated) next = { kind: 'truncated', key: k, size: blob.size, image };
      else if (blob.size === 0) next = { kind: 'empty', key: k };
      else if (image && blob.base64) {
        const mime = blob.mime ?? MIME[extname(p)] ?? 'application/octet-stream';
        next = { kind: 'image', key: k, url: `data:${mime};base64,${blob.base64}`, size: blob.size };
      } else if (blob.binary || blob.text == null) next = { kind: 'binary', key: k, size: blob.size };
      else {
        await showText(id, k, p, splitLines(blob.text));
        return;
      }
    } catch (e) {
      if (id !== seq) return;
      next = { kind: 'error', key: k, message: errorMessage(e) };
    }
    const prev = content;
    if (!(next.kind === 'image' && prev?.kind === 'image' && prev.url === next.url)) dims = null;
    content = next;
    tokens = null;
    loading = false;
    // Nothing to focus in images and placeholders: drop a pending request.
    focusReq = null;
  }

  async function showText(id: number, k: string, p: string, lines: string[]): Promise<void> {
    const prev = content;
    if (prev?.kind === 'text' && tokens && prev.path === p && sameLines(prev.lines, lines)) {
      // Unchanged text (e.g. a working-tree refresh): keep lines and tokens as they are.
      if (prev.key !== k) content = { ...prev, key: k };
      loading = false;
      return;
    }
    const hl = highlightFor(p, lines);
    const early = await within(hl, HL_WAIT);
    if (id !== seq) return;
    // Another file starts at its top-left; the same path at another revision keeps the position.
    const fresh = prev?.kind !== 'text' || prev.path !== p;
    content = { kind: 'text', key: k, path: p, lines, cols: maxColumns(lines) };
    tokens = early === PENDING ? null : early;
    loading = false;
    if (fresh || focusReq) {
      await tick();
      if (id !== seq) return;
      if (fresh) {
        list?.scrollToIndex(0, 'start');
        const vp = list?.getViewport();
        if (vp) vp.scrollLeft = 0;
      }
      serveFocus();
    }
    if (early === PENDING) {
      const late = await hl;
      if (id === seq) tokens = late;
    }
  }

  function onImageLoad(e: Event, c: Content): void {
    const img = e.currentTarget as HTMLImageElement;
    if (content === c) dims = `${img.naturalWidth}×${img.naturalHeight}`;
  }

  function onImageError(c: Content): void {
    if (content === c && c.kind === 'image') content = { kind: 'binary', key: c.key, size: c.size };
  }

  /**
   * Focus the text for keyboard scrolling. While no text is shown yet (loading), the request is
   * served when this file's text appears, unless focus moved meanwhile; another file, an image or a
   * placeholder drops it.
   */
  export function focus(): void {
    if (list) {
      focusReq = null;
      list.focus();
    } else focusReq = { key, from: document.activeElement };
  }

  function serveFocus(): void {
    const req = focusReq;
    if (!req) return;
    focusReq = null;
    const a = document.activeElement;
    if (list && content?.key === req.key && (a === req.from || !a || a === document.body)) list.focus();
  }

  function oncopy(e: ClipboardEvent): void {
    const c = content;
    if (c?.kind !== 'text' || !root) return;
    const text = selectedText(root, c.lines);
    if (text == null) return;
    e.clipboardData?.setData('text/plain', text);
    e.preventDefault();
  }
</script>

<svelte:document {oncopy} />

<div class="fv" bind:this={root}>
  {#if loading}<div class="progress"></div>{/if}
  {#if content?.kind === 'text'}
    {@const c = content}
    <div class="text" class:stale style:--digits={digits} style:--cols={c.cols}>
      <VirtualList bind:this={list} count={c.lines.length} rowHeight={LINE_H} overscan={16} horizontal tabindex={0}>
        {#snippet row(i)}
          <div class="line" data-line={i}>
            <span class="ln" data-n={i + 1}></span>
            <span class="code selectable" data-code
              ><DiffLineText text={c.lines[i] ?? ''} tokens={tokens?.[i] ?? null} ranges={null} /></span
            >
          </div>
        {/snippet}
      </VirtualList>
    </div>
  {:else if content?.kind === 'image'}
    {@const c = content}
    <div class="image" class:stale>
      <img src={c.url} alt={basename(path)} onload={(e) => onImageLoad(e, c)} onerror={() => onImageError(c)} />
      <span class="meta">{dims ? `${dims} · ` : ''}{formatBytes(c.size)}</span>
    </div>
  {:else if content}
    <div class="ph empty" class:stale>
      {#if content.kind === 'binary'}
        <FileQuestionMark size={28} strokeWidth={1.5} />
        <span class="meta">{formatBytes(content.size)}</span>
      {:else if content.kind === 'truncated'}
        {#if content.image}<FileImage size={28} strokeWidth={1.5} />{:else}<FileText size={28} strokeWidth={1.5} />{/if}
        <span class="meta">
          {formatBytes(content.size)}
          {#if content.size <= FULL_BYTES}
            {@const k = content.key}
            <button class="icon-btn" title="Load" onclick={() => (fullKey = k)}>
              <ArrowDownToLine size={14} strokeWidth={1.75} />
            </button>
          {/if}
        </span>
      {:else if content.kind === 'empty'}
        <span class="glyph">∅</span>
      {:else}
        <FileX size={28} strokeWidth={1.5} />
        <span class="meta msg selectable">{content.message}</span>
      {/if}
    </div>
  {/if}
</div>

<style>
  .fv {
    position: relative;
    height: 100%;
    min-height: 0;
    overflow: hidden;
    background: var(--bg-1);
  }
  .text {
    position: absolute;
    inset: 0;
    font-family: var(--font-mono);
    font-size: var(--fs-code);
    line-height: var(--line-h);
    --ln-w: calc(var(--digits) * 1ch + 20px);
  }
  .line {
    display: flex;
    height: 100%;
    min-width: calc(var(--ln-w) + var(--cols) * 1ch + 24px);
  }
  .ln {
    position: sticky;
    left: 0;
    z-index: 1;
    flex: none;
    width: var(--ln-w);
    padding-right: 12px;
    text-align: right;
    color: var(--text-2);
    background: var(--bg-1);
  }
  .ln::before {
    content: attr(data-n);
  }
  .code {
    flex: 1 0 auto;
    padding-left: 4px;
    white-space: pre;
    tab-size: 4;
    color: var(--shiki-foreground);
  }
  .image {
    position: absolute;
    inset: 0;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: var(--sp-2);
    padding: var(--sp-4);
  }
  .image img {
    max-width: 100%;
    max-height: calc(100% - 24px);
    min-height: 0;
    object-fit: contain;
    background: conic-gradient(var(--bg-2) 25%, var(--bg-3) 0 50%, var(--bg-2) 0 75%, var(--bg-3) 0) 0 0 / 16px 16px;
  }
  .ph {
    position: absolute;
    inset: 0;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: var(--sp-2);
    padding: var(--sp-4);
    color: var(--text-3);
  }
  .meta {
    display: inline-flex;
    align-items: center;
    gap: var(--sp-1);
    color: var(--text-2);
    font-size: var(--fs-xs);
    font-variant-numeric: tabular-nums;
  }
  .msg {
    max-width: 560px;
    text-align: center;
    white-space: pre-wrap;
    word-break: break-word;
  }
  .glyph {
    font-size: 20px;
    color: var(--text-3);
  }
  .stale {
    opacity: 0.55;
    transition: opacity 0s linear 150ms;
  }
  .progress {
    position: absolute;
    top: 0;
    left: 0;
    right: 0;
    height: 2px;
    z-index: 3;
    overflow: hidden;
    pointer-events: none;
    opacity: 0;
    animation: appear 0s linear 300ms forwards;
  }
  .progress::after {
    content: '';
    position: absolute;
    top: 0;
    bottom: 0;
    left: 0;
    width: 30%;
    background: var(--accent);
    animation: slide 1s ease-in-out infinite;
  }
  @keyframes appear {
    to {
      opacity: 1;
    }
  }
  @keyframes slide {
    from {
      transform: translateX(-100%);
    }
    to {
      transform: translateX(340%);
    }
  }
</style>
