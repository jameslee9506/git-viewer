<!--
  One stash, laid out like a commit: a fixed-height head (DetailsHead: the name / sha / base bar,
  the scrolling message box, the time), a summary of its changes, then its tracked changes and
  (stash -u) the untracked files recorded in its third parent.
-->
<script lang="ts">
  import CircleDot from '@lucide/svelte/icons/circle-dot';
  import CirclePlus from '@lucide/svelte/icons/circle-plus';
  import FolderTree from '@lucide/svelte/icons/folder-tree';
  import { getRepo } from '../../state/repo.svelte';
  import type { DiffItem } from '../../state/repo.svelte';
  import { settings } from '../../state/settings.svelte';
  import { toast } from '../../state/ui.svelte';
  import type { FileChange, Stash } from '../../types';
  import { absoluteTime, copyText, relativeTime, shortOid } from '../../util/format';
  import DetailsHead from './DetailsHead.svelte';
  import FileList, { changeSummary, fileBar, fileFilter, noMatch, sectionHead, sectionMinHeight } from './FileList.svelte';
  import type { FileListNav } from './FileList.svelte';
  import { displayOrder, entriesFromChanges, filterSections, lineTotals, statusCounts } from './filetree';
  import type { FileEntry } from './filetree';

  interface Props {
    oid: string;
    /** Stash entry when it was loaded (the live one from repo.refs wins). */
    stash: Stash | null;
    files: FileChange[];
    /** Files of the untracked-files commit, null when the stash has none. */
    untracked: FileChange[] | null;
  }

  let { oid, stash, files, untracked }: Props = $props();

  type SectionKind = 'tracked' | 'untracked';

  interface Section {
    kind: SectionKind;
    title: string;
    icon: typeof CircleDot;
    entries: FileEntry[];
  }

  const repo = getRepo();
  const filter = fileFilter(repo);
  /** Height of the view: bounds the head. */
  let panelH = $state(0);

  const entry = $derived(repo.refs?.stashes.find((s) => s.oid === oid) ?? stash);
  /** Every section; a lone one is shown without its header. */
  const allSections = $derived.by((): Section[] => {
    const out: Section[] = [];
    if (files.length > 0 || !untracked || untracked.length === 0) {
      out.push({
        kind: 'tracked',
        title: 'Changes',
        icon: CircleDot,
        entries: entriesFromChanges({ kind: 'stash', oid }, files),
      });
    }
    if (untracked && untracked.length > 0) {
      out.push({
        kind: 'untracked',
        title: 'Untracked',
        icon: CirclePlus,
        entries: entriesFromChanges({ kind: 'stashUntracked', oid }, untracked),
      });
    }
    return out;
  });
  /** The sections with the files the path filter leaves (empty ones dropped). */
  const sections = $derived(filterSections(allSections, filter.terms));
  /** Unfiltered size of each section: its header shows matched/total while filtering. */
  const sizes = $derived(new Map(allSections.map((s) => [s.kind, s.entries.length])));
  const all = $derived(untracked ? [...files, ...untracked] : files);
  const shownCount = $derived(sections.reduce((n, s) => n + s.entries.length, 0));
  const totals = $derived(lineTotals(all));
  /** Untracked files count as untracked (the backend lists them with status `?`). */
  const counts = $derived(statusCounts(all));

  let collapsed = $state.raw<readonly SectionKind[]>([]);

  /** The file lists on screen: the lone section's, else each expanded section's. */
  let single = $state.raw<FileListNav | null>();
  const lists: Partial<Record<SectionKind, FileListNav | null>> = {};

  function toggle(kind: SectionKind): void {
    collapsed = collapsed.includes(kind) ? collapsed.filter((k) => k !== kind) : [...collapsed, kind];
  }

  /** The first file list on screen (the filter box's ↓ / Enter). */
  function firstList(): FileListNav | null | undefined {
    if (allSections.length === 1) return single;
    const sec = sections.find((s) => !collapsed.includes(s.kind));
    return sec && lists[sec.kind];
  }

  /** Diff siblings span both sections in display order; position of each entry in that list (cached). */
  const siblings = $derived.by(() => {
    const items: DiffItem[] = [];
    const pos = new Map<SectionKind, Int32Array>();
    for (const s of sections) {
      const at = new Int32Array(s.entries.length);
      for (const k of displayOrder(s.entries, settings.fileListMode)) {
        at[k] = items.length;
        items.push(s.entries[k].item);
      }
      pos.set(s.kind, at);
    }
    return { items, pos };
  });

  function openFrom(section: Section, index: number): void {
    const { items, pos } = siblings;
    repo.openDiff(items, pos.get(section.kind)?.[index] ?? 0);
  }

  function copy(text: string): void {
    void copyText(text).then(() => toast('Copied'));
  }
</script>

{#snippet browse()}
  <button
    class="icon-btn"
    title="Browse files"
    onclick={() => repo.openView({ kind: 'tree', rev: oid, path: null })}
  >
    <FolderTree size={15} strokeWidth={1.75} />
  </button>
{/snippet}

<div class="stash" bind:clientHeight={panelH}>
  <DetailsHead panelHeight={panelH}>
    {#snippet bar()}
      <div class="hbar">
        <span class="lbl">stash</span>
        {#if entry}<span class="sname ellipsis">{entry.name}</span>{/if}
        <button class="sha" class:alt={!!entry} title={oid} onclick={() => copy(oid)}>{shortOid(oid)}</button>
        {#if entry}
          {@const base = entry.base}
          <span class="base">
            <span class="lbl">base</span>
            <button class="sha link" title={base} onclick={() => void repo.reveal(base)}>{shortOid(base)}</button>
          </span>
        {/if}
      </div>
    {/snippet}
    {#snippet message()}
      {#if entry?.message}
        <div class="subject selectable">{entry.message}</div>
      {/if}
    {/snippet}
    {#snippet meta()}
      {#if entry}
        <div class="when">
          <span class="lbl">stashed</span>
          <span class="abs">{absoluteTime(entry.time)}</span>
          <span class="rel">· {relativeTime(entry.time)}</span>
        </div>
      {/if}
    {/snippet}
  </DetailsHead>

  {@render changeSummary({ counts, additions: totals.additions, deletions: totals.deletions })}
  {@render fileBar({ count: all.length, shown: shownCount, filter, list: firstList }, undefined, browse)}

  {#if sections.length === 0}
    {@render noMatch()}
  {:else if allSections.length === 1}
    <FileList bind:this={single} entries={sections[0].entries} filter={filter.key} />
  {:else}
    <div class="sections">
      {#each sections as sec (sec.kind)}
        {@const closed = collapsed.includes(sec.kind)}
        <section class="sec" class:closed style:min-height={closed ? null : sectionMinHeight(sec.entries.length)}>
          {@render sectionHead({
            title: sec.title,
            icon: sec.icon,
            count: filter.key ? `${sec.entries.length}/${sizes.get(sec.kind)}` : `${sec.entries.length}`,
            closed,
            ontoggle: () => toggle(sec.kind),
          })}
          {#if !closed}
            <FileList
              bind:this={lists[sec.kind]}
              entries={sec.entries}
              filter={filter.key}
              onopen={(i) => openFrom(sec, i)}
            />
          {/if}
        </section>
      {/each}
    </div>
  {/if}
</div>

<style>
  .stash {
    flex: 1 1 0;
    min-height: 0;
    display: flex;
    flex-direction: column;
    overflow-x: hidden;
    overflow-y: auto;
  }

  .hbar {
    flex: none;
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 2px 6px;
    min-height: var(--toolbar-h);
    padding: 4px 8px 4px 12px;
    border-bottom: 1px solid var(--border);
  }
  .lbl {
    flex: none;
    color: var(--text-2);
    font-size: var(--fs-sm);
  }
  .sname {
    flex: 0 1 auto;
    color: var(--text-0);
    font-family: var(--font-mono);
    font-size: var(--fs-code);
  }
  .base {
    display: inline-flex;
    align-items: center;
    gap: 2px;
    margin-left: auto;
  }
  .base .lbl {
    margin-right: 4px;
  }
  .sha {
    flex: none;
    height: 24px;
    padding: 0 5px;
    border-radius: var(--radius-sm);
    color: var(--text-0);
    font-family: var(--font-mono);
    font-size: var(--fs-code);
  }
  /* Next to the stash name the sha is the secondary id. */
  .sha.alt {
    color: var(--text-1);
  }
  .sha:hover {
    background: var(--bg-hover);
    color: var(--text-0);
  }
  .sha.link {
    color: var(--link);
  }
  .sha.link:hover {
    color: color-mix(in srgb, var(--link) 70%, var(--text-0));
  }

  /* In DetailsHead's message box (one subject line = MSG_MIN there). */
  .subject {
    color: var(--text-0);
    font-size: var(--fs-lg);
    font-weight: 500;
    line-height: 1.4;
    white-space: pre-wrap;
    overflow-wrap: anywhere;
  }

  .when {
    flex: none;
    display: flex;
    align-items: center;
    gap: 6px;
    min-width: 0;
    padding: 12px;
    font-size: var(--fs-sm);
    line-height: 18px;
    white-space: nowrap;
  }
  .abs {
    color: var(--text-1);
  }
  .rel {
    color: var(--text-2);
  }

  .sections {
    flex: 1 1 0;
    min-height: 0;
    display: flex;
    flex-direction: column;
    overflow-y: auto;
  }
  .sec {
    flex: 0 1 auto;
    display: flex;
    flex-direction: column;
    min-height: 0;
    border-bottom: 1px solid var(--border);
  }
  .sec.closed {
    flex: none;
  }
</style>
