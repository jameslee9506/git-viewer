<!--
  Two-up image diff on a checkerboard with a dim `W×H · size` under each side (outline tinted with the
  deleted / added gutter color). Added files show only the new side, deleted only the old one.
  A side is read again only when its key (file, status and the diff's blob id for that side) changes:
  the parent refetches working-tree diffs and passes their ids, so a refetch that finds the same diff
  reads nothing, and a working-tree edit (which changes only `newOid`) re-reads only the new side.
-->
<script lang="ts">
  import ImageOff from '@lucide/svelte/icons/image-off';
  import { untrack } from 'svelte';
  import { api, errorMessage } from '../../api';
  import { getRepo } from '../../state/repo.svelte';
  import type { BlobSpec, ChangeStatus, DiffTarget } from '../../types';
  import { extname, formatBytes } from '../../util/format';
  import { sideSpecs } from './model';

  interface Props {
    target: DiffTarget;
    path: string;
    oldPath: string | null;
    status: ChangeStatus;
    /** The diff's old blob id (`FileDiff.oldOid`); when set, the old side is read as exactly this blob. */
    oldOid?: string | null;
    /**
     * The diff's new blob id (`FileDiff.newOid`; for a working-tree file, git's hash of its content):
     * the new side is read again when it changes.
     */
    newOid?: string | null;
  }

  let { target, path, oldPath, status, oldOid = null, newOid = null }: Props = $props();

  interface Side {
    /** data: URL; null when the blob could not be read or exceeds the read limit. */
    url: string | null;
    size: number | null;
    error: string | null;
  }

  type Which = 'old' | 'new';

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

  const repo = getRepo();

  let oldSide = $state.raw<Side | null>(null);
  let newSide = $state.raw<Side | null>(null);
  /** Natural size text per side, valid for the Side object it was measured on. */
  let dims = $state.raw<Record<Which, [Side, string] | null>>({ old: null, new: null });
  /** Sides whose data failed to decode. */
  let broken = $state.raw<Side[]>([]);

  /** What each side shows: everything its blob spec and bytes depend on. */
  const oldKey = $derived(JSON.stringify([target, oldPath ?? path, status, oldOid]));
  const newKey = $derived(JSON.stringify([target, path, status, newOid]));
  /** Key of each side's latest read, and that read's id. */
  const requested: Record<Which, string | null> = { old: null, new: null };
  const seq: Record<Which, number> = { old: 0, new: 0 };

  // Read the sides whose key changed. Sides read together are applied together, so switching to
  // another image swaps both panes at once.
  $effect(() => {
    const keys: Record<Which, string> = { old: oldKey, new: newKey };
    untrack(() => {
      const stale = (['old', 'new'] as const).filter((w) => keys[w] !== requested[w]);
      for (const w of stale) requested[w] = keys[w];
      if (stale.length > 0) void load(stale);
    });
  });

  function read(spec: BlobSpec | null, file: string): Promise<Side | null> {
    if (!spec) return Promise.resolve(null);
    return api.readBlob(repo.id, spec, 'base64').then(
      (b) => ({
        url: b.base64 && !b.truncated ? `data:${b.mime ?? MIME[extname(file)] ?? 'image/png'};base64,${b.base64}` : null,
        size: b.size,
        error: null,
      }),
      (e: unknown) => ({ url: null, size: null, error: errorMessage(e) }),
    );
  }

  /** Keep the previous object when nothing changed, so the image and its measured size stay. */
  function keep(prev: Side | null, next: Side | null): Side | null {
    if (prev && next && prev.url === next.url && prev.size === next.size && prev.error === next.error) return prev;
    return next;
  }

  async function load(sides: Which[]): Promise<void> {
    const ids = sides.map((w) => ++seq[w]);
    const specs = sideSpecs(target, path, oldPath, status, repo.id, oldOid);
    // The old side is always a stored blob: read the one the diff names, so its bytes match its key. The
    // new side stays as specified: a working-tree file's id is only a hash, not a stored object.
    const spec: Record<Which, BlobSpec | null> = {
      old: specs.old && oldOid ? { kind: 'oid', oid: oldOid } : specs.old,
      new: specs.new,
    };
    const got = await Promise.all(sides.map((w) => read(spec[w], w === 'old' ? (oldPath ?? path) : path)));
    sides.forEach((w, i) => {
      if (ids[i] !== seq[w]) return;
      if (w === 'old') oldSide = keep(oldSide, got[i]);
      else newSide = keep(newSide, got[i]);
    });
  }

  function measure(which: Which, s: Side, img: HTMLImageElement) {
    dims = { ...dims, [which]: [s, `${img.naturalWidth}×${img.naturalHeight}`] };
  }

  function caption(s: Side, which: Which): string {
    const d = dims[which];
    return [d && d[0] === s ? d[1] : '', s.size != null ? formatBytes(s.size) : ''].filter(Boolean).join(' · ');
  }
</script>

<div class="img2">
  {#if oldSide}{@render pane(oldSide, 'old')}{/if}
  {#if newSide}{@render pane(newSide, 'new')}{/if}
</div>

{#snippet pane(s: Side, which: Which)}
  <figure class="pane {which}">
    <div class="stage">
      {#if s.url && !broken.includes(s)}
        <img
          src={s.url}
          alt=""
          draggable="false"
          onload={(e) => measure(which, s, e.currentTarget as HTMLImageElement)}
          onerror={() => (broken = [...broken.slice(-7), s])}
        />
      {:else}
        <span class="miss" title={s.error ?? undefined}><ImageOff size={22} strokeWidth={1.5} /></span>
      {/if}
    </div>
    <figcaption class="mono">{caption(s, which)}</figcaption>
  </figure>
{/snippet}

<style>
  .img2 {
    display: flex;
    gap: var(--sp-4);
    height: 100%;
    padding: var(--sp-4);
  }
  .pane {
    flex: 1 1 0;
    min-width: 0;
    margin: 0;
    display: flex;
    flex-direction: column;
    gap: var(--sp-2);
  }
  .stage {
    position: relative;
    flex: 1 1 auto;
    min-height: 0;
  }
  .stage img {
    position: absolute;
    inset: 0;
    margin: auto;
    max-width: 100%;
    max-height: 100%;
    background: repeating-conic-gradient(var(--bg-2) 0 25%, var(--bg-3) 0 50%) 0 0 / 16px 16px;
  }
  .miss {
    position: absolute;
    inset: 0;
    display: flex;
    align-items: center;
    justify-content: center;
    color: var(--text-3);
  }
  figcaption {
    flex: none;
    min-height: 16px;
    text-align: center;
    font-size: var(--fs-xs);
    color: var(--text-2);
  }
  .old img {
    box-shadow: 0 0 0 1px var(--del-gutter);
  }
  .new img {
    box-shadow: 0 0 0 1px var(--add-gutter);
  }
</style>
