<!--
  Fetches and renders one file diff: unified or split per settings, highlighted asynchronously (plain
  first), with placeholders for too-large, binary, image, submodule, nested-repo, empty, mode-only and
  rename-only diffs. Working-tree targets refetch when `repo.worktreeVersion` changes. Also used by
  FileHistoryView. Exported: step(dir) = next/previous change block, focus().
-->
<script lang="ts" module>
  import { loadJSON, saveJSON } from '../../state/persist';

  /** Diff preferences that are not part of Settings: show the whole file instead of hunks. */
  export const diffPrefs = $state({ fullFile: loadJSON<boolean>('diff.fullFile', false) === true });

  export function setFullFile(on: boolean): void {
    diffPrefs.fullFile = on;
    saveJSON('diff.fullFile', on);
  }

  /** Context lines requested for "full file". */
  const FULL_CONTEXT = 100000;

  /** How often a request that is still the newest is sent again after the backend cancelled it (see load). */
  const CANCEL_RETRIES = 3;

  /** Advance of one monospace code character in px (per instance: user.css may change the code font). */
  function measureCharWidth(): number {
    const probe = document.createElement('span');
    probe.style.cssText =
      'position:absolute;left:-10000px;top:0;visibility:hidden;white-space:pre;' +
      'font-family:var(--font-mono);font-size:var(--fs-code);font-variant-ligatures:none';
    probe.textContent = '0'.repeat(200);
    document.body.appendChild(probe);
    const w = probe.getBoundingClientRect().width / 200;
    probe.remove();
    return w > 0 ? w : 7.2;
  }
</script>

<script lang="ts">
  import ArrowRightLeft from '@lucide/svelte/icons/arrow-right-left';
  import Binary from '@lucide/svelte/icons/binary';
  import Boxes from '@lucide/svelte/icons/boxes';
  import Download from '@lucide/svelte/icons/download';
  import Equal from '@lucide/svelte/icons/equal';
  import FileIcon from '@lucide/svelte/icons/file';
  import FileCog from '@lucide/svelte/icons/file-cog';
  import FolderGit2 from '@lucide/svelte/icons/folder-git-2';
  import TriangleExclamationPoint from '@lucide/svelte/icons/triangle-exclamation-point';
  import WholeWord from '@lucide/svelte/icons/whole-word';
  import { onDestroy, untrack } from 'svelte';
  import { api, errorMessage } from '../../api';
  import { highlightLines, languageForPath, type Token } from '../../highlight';
  import { getFileDiff, isImmutableTarget } from '../../state/cache';
  import { diffItemKey, getRepo } from '../../state/repo.svelte';
  import { settings } from '../../state/settings.svelte';
  import type { BlobSpec, ChangeStatus, DiffRequest, DiffTarget, FileDiff } from '../../types';
  import { extname, formatBytes, isImagePath, shortOid } from '../../util/format';
  import ImageDiff from './ImageDiff.svelte';
  import SplitDiff from './SplitDiff.svelte';
  import UnifiedDiff from './UnifiedDiff.svelte';
  import { buildModel, sameDiff, sideSpecs, submoduleShas, type DiffModel } from './model';

  interface Props {
    target: DiffTarget;
    path: string;
    oldPath: string | null;
    status: ChangeStatus;
    /** Called with every newly shown diff (null when loading failed). */
    onloaded?: (diff: FileDiff | null) => void;
  }

  let { target, path, oldPath, status, onloaded }: Props = $props();

  interface Shown {
    /** Request identity (item + context + whitespace + size limit). */
    key: string;
    /** diffItemKey of the item: scroll positions are remembered per item. */
    itemKey: string;
    target: DiffTarget;
    path: string;
    oldPath: string | null;
    status: ChangeStatus;
    /** Requested with ignore-whitespace. */
    ws: boolean;
    diff: FileDiff;
  }

  interface Placeholder {
    icon: typeof Binary;
    text: string;
    mono?: boolean;
    title?: string;
  }

  /** Highlighting of one side of a model (tokens null: unsupported, too big or failed). */
  interface Hl {
    model: DiffModel;
    /** Shown.itemKey of the model's diff. */
    item: string;
    tokens: Token[][] | null;
  }

  const repo = getRepo();
  const charWidth = measureCharWidth();

  let shown = $state.raw<Shown | null>(null);
  let failed = $state.raw<string | null>(null);
  let pending = $state(false);
  /** Item key whose too-large diff the user asked to load anyway. */
  let forced = $state<string | null>(null);
  let sizes = $state.raw<{ of: Shown; old: number | null; new: number | null } | null>(null);
  let hlOld = $state.raw<Hl | null>(null);
  let hlNew = $state.raw<Hl | null>(null);
  /** focus() asked for while no diff list was shown: for which item, and where focus was at that time. */
  let focusReq = $state.raw<{ item: string; from: Element | null } | null>(null);
  let uniView: UnifiedDiff | undefined = $state();
  let splitView: SplitDiff | undefined = $state();
  let seq = 0;
  let hlSeq = 0;
  let sizeSeq = 0;

  const live = $derived(!isImmutableTarget(target));
  const itemKey = $derived(diffItemKey({ target, file: { path, oldPath, status } }));
  const context = $derived(diffPrefs.fullFile ? FULL_CONTEXT : settings.contextLines);
  const reqKey = $derived(
    [itemKey, oldPath ?? '', status, context, settings.ignoreWhitespace ? 'w' : '', forced === itemKey ? 'all' : ''].join(
      '\0',
    ),
  );

  const diff = $derived(shown?.diff ?? null);
  const sub = $derived(diff ? submoduleShas(diff) : null);
  const image = $derived(
    !!shown && shown.diff.binary && isImagePath(shown.path) && extname(shown.path) !== 'svg',
  );
  const model = $derived(
    diff && !diff.binary && !diff.tooLarge && !sub && diff.hunks.length > 0 ? buildModel(diff) : null,
  );
  const oldTokens = $derived(sideTokens(hlOld, model, shown?.itemKey, 'old'));
  const newTokens = $derived(sideTokens(hlNew, model, shown?.itemKey, 'new'));
  const needsSizes = $derived(!!shown && !model && (shown.diff.tooLarge || (shown.diff.binary && !image && !sub)));
  const curSizes = $derived(sizes && sizes.of === shown ? sizes : null);
  /** Size shown on the too-large placeholder: the new side, else the old one. */
  const largeSize = $derived(curSizes ? (curSizes.new ?? curSizes.old) : null);
  const placeholder = $derived(shown && !model && !image && !shown.diff.tooLarge ? describe(shown) : null);

  // Fetch whenever the request changes; working-tree targets also on every worktree change.
  $effect(() => {
    const key = reqKey;
    if (live) void repo.worktreeVersion;
    untrack(() => load(key));
  });

  // Highlight both sides (new first: it carries context + added lines), apply each when ready.
  $effect(() => {
    const m = model;
    const s = shown;
    const id = ++hlSeq;
    if (!m || !s) return;
    void highlight(m, s.itemKey, id, languageForPath(s.oldPath ?? s.path), languageForPath(s.path));
  });

  // Blob sizes for the binary / too-large placeholders (refreshed on worktree changes).
  $effect(() => {
    const s = shown;
    if (!s || !needsSizes) return;
    if (live) void repo.worktreeVersion;
    untrack(() => loadSizes(s));
  });

  // A pending focus request waits for the diff being loaded (or the first one, before its load starts)
  // and for its list, which binds after this effect first runs (the effect re-runs once it does). It is
  // served if that diff is of the requested item and focus did not move meanwhile: still where it was
  // (e.g. FileHistoryView's list, whose → asked for it), or on the page / in an inert layer. Another
  // item, a placeholder or an error drops it.
  $effect(() => {
    const req = focusReq;
    if (!req) return;
    const v = settings.diffMode === 'split' ? splitView : uniView;
    if (pending || (model && !v) || (!shown && failed === null)) return;
    untrack(() => {
      focusReq = null;
      const a = document.activeElement;
      if (v && shown?.itemKey === req.item && (a === req.from || focusIsFree())) v.focus();
    });
  });

  // A new list (unified ↔ split, or a diff after a placeholder) replaces one that may have had focus:
  // the focus fell back to the page, so hand it to the new list (nothing else claimed it).
  $effect(() => {
    const v = settings.diffMode === 'split' ? splitView : uniView;
    if (!v) return;
    untrack(() => {
      if (focusIsFree()) v.focus();
    });
  });

  // Requests in flight when the view goes drop their result and never retry: a retry would cancel the
  // run of a newer view of the same working-tree file.
  onDestroy(() => {
    seq++;
  });

  function load(key: string, retries = CANCEL_RETRIES): void {
    const id = ++seq;
    const snap = { key, itemKey, target, path, oldPath, status, ws: settings.ignoreWhitespace };
    const req: DiffRequest = {
      target,
      path,
      oldPath,
      context,
      ignoreWhitespace: settings.ignoreWhitespace,
      maxBytes: forced === itemKey ? 0 : null,
    };
    pending = true;
    getFileDiff(repo.id, req).then(
      (d) => {
        if (id !== seq) return;
        pending = false;
        failed = null;
        const cur = shown;
        if (cur && cur.key === key && sameDiff(cur.diff, d)) return;
        shown = { ...snap, diff: d };
        onloaded?.(d);
      },
      (e: unknown) => {
        if (id !== seq) return;
        const message = errorMessage(e);
        // A working-tree diff is cancelled by a newer request for the same file. This one is still the
        // newest: an older request reached the backend after it, so ask again (that kills the older run).
        if (message === 'cancelled' && retries > 0) {
          load(key, retries - 1);
          return;
        }
        pending = false;
        shown = null;
        failed = message;
        onloaded?.(null);
      },
    );
  }

  async function highlight(m: DiffModel, item: string, id: number, langOld: string | null, langNew: string | null) {
    if (langNew && m.newLines.length > 0) {
      const t = await highlightLines(langNew, m.newLines, m.newResets).catch(() => null);
      if (id !== hlSeq) return;
      hlNew = { model: m, item, tokens: t };
    }
    if (langOld && m.oldLines.length > 0) {
      const t = await highlightLines(langOld, m.oldLines, m.oldResets).catch(() => null);
      if (id !== hlSeq) return;
      hlOld = { model: m, item, tokens: t };
    }
  }

  /**
   * Tokens of one side of the shown model. While a newer diff of the same file (working-tree refetch,
   * context or whitespace toggle) is being highlighted, lines identical to lines of the previous diff
   * keep their tokens instead of flashing plain.
   */
  function sideTokens(h: Hl | null, m: DiffModel | null, item: string | undefined, side: 'old' | 'new'): Token[][] | null {
    if (!h || !m) return null;
    if (h.model === m) return h.tokens;
    if (!h.tokens || h.item !== item) return null;
    const prev = side === 'old' ? h.model.oldLines : h.model.newLines;
    const byText = new Map<string, Token[]>();
    for (let i = 0; i < prev.length; i++) {
      const t = h.tokens[i];
      if (t) byText.set(prev[i], t);
    }
    return (side === 'old' ? m.oldLines : m.newLines).map((line) => byText.get(line) ?? []);
  }

  async function loadSizes(s: Shown): Promise<void> {
    const id = ++sizeSeq;
    const specs = sideSpecs(s.target, s.path, s.oldPath, s.status, repo.id, s.diff.oldOid);
    const size = (spec: BlobSpec | null): Promise<number | null> =>
      spec
        ? api.readBlob(repo.id, spec, 'text', 1).then(
            (b) => b.size,
            () => null,
          )
        : Promise.resolve(null);
    const [o, n] = await Promise.all([size(specs.old), size(specs.new)]);
    if (id !== sizeSeq) return;
    sizes = { of: s, old: o, new: n };
  }

  function sizeText(o: number | null, n: number | null): string {
    if (o != null && n != null) return `${formatBytes(o)} → ${formatBytes(n)}`;
    if (o != null) return formatBytes(o);
    if (n != null) return formatBytes(n);
    return '';
  }

  function shaText(sha: string | null): string {
    if (!sha) return '∅';
    const dirty = sha.endsWith('-dirty');
    return shortOid(dirty ? sha.slice(0, -6) : sha) + (dirty ? '-dirty' : '');
  }

  /** Placeholder for a diff that has no text hunks to show. */
  function describe(s: Shown): Placeholder {
    const d = s.diff;
    if (d.headerLines.includes('nested repository')) return { icon: FolderGit2, text: '' };
    if (sub) {
      const text = sub.old && sub.new ? `${shaText(sub.old)} → ${shaText(sub.new)}` : shaText(sub.new ?? sub.old);
      return { icon: Boxes, text, mono: true, title: [sub.old, sub.new].filter(Boolean).join(' → ') };
    }
    if (d.binary) {
      const z = curSizes;
      return { icon: Binary, text: z ? sizeText(z.old, z.new) : '', mono: true };
    }
    const parts: string[] = [];
    let icon: typeof Binary = Equal;
    const copied = d.headerLines.some((l) => l.startsWith('copy from '));
    const renamed = copied || d.headerLines.some((l) => l.startsWith('rename from ')) || s.status === 'R';
    if (renamed) {
      const sim = d.headerLines.map((l) => /^similarity index (\d+)%/.exec(l)).find((m) => m !== null);
      parts.push(`${copied || s.status === 'C' ? 'C' : 'R'}${sim ? sim[1] : ''}`);
      icon = ArrowRightLeft;
    }
    if (d.oldMode && d.newMode && d.oldMode !== d.newMode) {
      parts.push(`${d.oldMode} → ${d.newMode}`);
      if (!renamed) icon = FileCog;
    }
    if (parts.length > 0) return { icon, text: parts.join(' · '), mono: true };
    // Unmerged path whose working-tree file equals "ours", or deleted/added on one side (DU, UD, AU, UA).
    if (s.status === 'U') return { icon: TriangleExclamationPoint, text: '' };
    const created = d.headerLines.some((l) => l.startsWith('new file mode') || l.startsWith('deleted file mode'));
    if (created || s.status === 'A' || s.status === 'D' || s.status === '?') return { icon: FileIcon, text: '∅' };
    return { icon: s.ws ? WholeWord : Equal, text: '' };
  }

  function focusIsFree(): boolean {
    const a = document.activeElement;
    return !a || a === document.body || a.closest('[inert]') !== null;
  }

  /** Jump to the next (1) / previous (-1) change block. */
  export function step(dir: 1 | -1): void {
    (settings.diffMode === 'split' ? splitView : uniView)?.step(dir);
  }

  /**
   * Focus the diff list. While none is shown (loading, or a placeholder until the next diff), the
   * request is served when this item's diff list appears, unless focus moved meanwhile.
   */
  export function focus(): void {
    const v = settings.diffMode === 'split' ? splitView : uniView;
    if (v) {
      focusReq = null;
      v.focus();
    } else focusReq = { item: itemKey, from: document.activeElement };
  }
</script>

<div class="dc">
  {#if pending}<div class="progress"></div>{/if}
  {#if failed !== null}
    <div class="ph">
      <TriangleExclamationPoint size={22} strokeWidth={1.5} />
      <span class="line selectable">{failed}</span>
    </div>
  {:else if shown}
    {@const s = shown}
    {#if model}
      {#if settings.diffMode === 'split'}
        <SplitDiff
          bind:this={splitView}
          {model}
          {oldTokens}
          {newTokens}
          {charWidth}
          scrollKey={s.itemKey}
          viewKey={s.key}
        />
      {:else}
        <UnifiedDiff
          bind:this={uniView}
          {model}
          {oldTokens}
          {newTokens}
          {charWidth}
          scrollKey={s.itemKey}
          viewKey={s.key}
        />
      {/if}
    {:else if image}
      <!-- Re-reads a side only when the shown diff names another blob for it (an unchanged refetch keeps `s`). -->
      <ImageDiff
        target={s.target}
        path={s.path}
        oldPath={s.oldPath}
        status={s.status}
        oldOid={s.diff.oldOid}
        newOid={s.diff.newOid}
      />
    {:else if s.diff.tooLarge}
      <div class="ph">
        <button class="icon-btn load" title="Load" onclick={() => (forced = s.itemKey)}>
          <Download size={18} strokeWidth={1.5} />
        </button>
        {#if largeSize !== null}
          <span class="line mono">{formatBytes(largeSize)}</span>
        {/if}
      </div>
    {:else if placeholder}
      <div class="ph" title={placeholder.title}>
        <placeholder.icon size={22} strokeWidth={1.5} />
        {#if placeholder.text}
          <span class="line" class:mono={placeholder.mono}>{placeholder.text}</span>
        {/if}
      </div>
    {/if}
  {/if}
</div>

<style>
  .dc {
    position: relative;
    height: 100%;
    min-height: 0;
    background: var(--bg-1);
  }
  .progress {
    position: absolute;
    top: 0;
    left: 0;
    right: 0;
    height: 1px;
    z-index: 3;
    opacity: 0;
    pointer-events: none;
    background: linear-gradient(90deg, transparent, var(--accent), transparent) 0 0 / 40% 100% no-repeat;
    animation:
      appear 0s linear 150ms forwards,
      sweep 1.1s linear infinite;
  }
  @keyframes appear {
    to {
      opacity: 1;
    }
  }
  @keyframes sweep {
    from {
      background-position: -40% 0;
    }
    to {
      background-position: 140% 0;
    }
  }
  .ph {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 10px;
    height: 100%;
    padding: 0 var(--sp-4);
    color: var(--text-3);
  }
  .line {
    max-width: 100%;
    font-size: var(--fs-sm);
    color: var(--text-2);
    text-align: center;
    overflow-wrap: anywhere;
  }
  .line.mono {
    font-family: var(--font-mono);
    font-size: var(--fs-code);
  }
  .load {
    width: 32px;
    height: 32px;
    color: var(--text-1);
  }
</style>
