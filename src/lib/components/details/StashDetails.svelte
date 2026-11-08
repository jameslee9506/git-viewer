<!--
  One stash: name, time, message, sha ← base, then its tracked changes and (stash -u) the
  untracked files recorded in its third parent.
-->
<script lang="ts">
  import Archive from '@lucide/svelte/icons/archive';
  import ArrowLeft from '@lucide/svelte/icons/arrow-left';
  import ChevronDown from '@lucide/svelte/icons/chevron-down';
  import ChevronRight from '@lucide/svelte/icons/chevron-right';
  import CircleDot from '@lucide/svelte/icons/circle-dot';
  import CirclePlus from '@lucide/svelte/icons/circle-plus';
  import FolderTree from '@lucide/svelte/icons/folder-tree';
  import GitCommitHorizontal from '@lucide/svelte/icons/git-commit-horizontal';
  import { getRepo } from '../../state/repo.svelte';
  import type { DiffItem } from '../../state/repo.svelte';
  import { settings } from '../../state/settings.svelte';
  import { toast } from '../../state/ui.svelte';
  import type { FileChange, Stash } from '../../types';
  import { absoluteTime, copyText, formatTime, shortOid } from '../../util/format';
  import FileList, { fileBar, fileFilter, noMatch } from './FileList.svelte';
  import type { FileListNav } from './FileList.svelte';
  import { displayOrder, entriesFromChanges, filterSections, lineTotals } from './filetree';
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
  const count = $derived(files.length + (untracked?.length ?? 0));
  const shownCount = $derived(sections.reduce((n, s) => n + s.entries.length, 0));
  const totals = $derived(lineTotals(untracked ? [...files, ...untracked] : files));

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
    <FolderTree size={14} strokeWidth={1.75} />
  </button>
{/snippet}

<div class="stash">
  <div class="top">
    <span class="ic"><Archive size={14} strokeWidth={1.75} /></span>
    <span class="name">{entry?.name ?? shortOid(oid)}</span>
    {#if entry}
      <span class="time" title={absoluteTime(entry.time)}>{formatTime(entry.time, settings.dateFormat)}</span>
    {/if}
  </div>
  {#if entry?.message}
    <div class="msg selectable">{entry.message}</div>
  {/if}
  <div class="shas">
    <span class="ic"><GitCommitHorizontal size={14} strokeWidth={1.75} /></span>
    <button class="sha" title={oid} onclick={() => copy(oid)}>{shortOid(oid)}</button>
    {#if entry}
      {@const base = entry.base}
      <span class="ic"><ArrowLeft size={12} strokeWidth={1.75} /></span>
      <button class="sha base" title={base} onclick={() => void repo.reveal(base)}>{shortOid(base)}</button>
    {/if}
  </div>

  {@render fileBar(
    {
      count,
      shown: shownCount,
      additions: totals.additions,
      deletions: totals.deletions,
      filter,
      list: firstList,
    },
    undefined,
    browse,
  )}

  {#if sections.length === 0}
    {@render noMatch()}
  {:else if allSections.length === 1}
    <FileList bind:this={single} entries={sections[0].entries} filter={filter.key} />
  {:else}
    <div class="sections">
      {#each sections as sec (sec.kind)}
        {@const closed = collapsed.includes(sec.kind)}
        {@const Icon = sec.icon}
        <section
          class="sec"
          class:closed
          style:min-height={closed ? null : `${27 + Math.min(sec.entries.length, 3) * 22}px`}
        >
          <button class="sec-h" title={sec.title} onclick={() => toggle(sec.kind)}>
            <span class="chev">
              {#if closed}
                <ChevronRight size={12} strokeWidth={1.75} />
              {:else}
                <ChevronDown size={12} strokeWidth={1.75} />
              {/if}
            </span>
            <Icon size={13} strokeWidth={1.75} />
            <span class="num">{filter.key ? `${sec.entries.length}/${sizes.get(sec.kind)}` : sec.entries.length}</span>
          </button>
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
  .top {
    flex: none;
    display: flex;
    align-items: center;
    gap: 6px;
    min-width: 0;
    padding: 12px 12px 4px;
    font-size: var(--fs-sm);
  }
  .ic {
    display: inline-flex;
    flex: none;
    color: var(--text-2);
  }
  .name {
    color: var(--text-1);
    font-family: var(--font-mono);
    font-size: var(--fs-xs);
  }
  .time {
    flex: none;
    margin-left: auto;
    color: var(--text-2);
    font-size: var(--fs-xs);
  }
  .msg {
    flex: none;
    max-height: 40%;
    overflow-y: auto;
    padding: 2px 12px 6px;
    color: var(--text-0);
    font-size: var(--fs);
    font-weight: 600;
    line-height: 1.35;
    white-space: pre-wrap;
    overflow-wrap: anywhere;
  }
  .shas {
    flex: none;
    display: flex;
    align-items: center;
    gap: 4px;
    height: 20px;
    margin: 0 12px 10px;
  }
  .sha {
    flex: none;
    height: 18px;
    padding: 0 4px;
    border-radius: var(--radius-sm);
    color: var(--text-1);
    font-family: var(--font-mono);
    font-size: var(--fs-xs);
  }
  .sha:hover {
    background: var(--bg-hover);
    color: var(--text-0);
  }
  .sha.base {
    color: var(--accent);
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
  .sec-h {
    flex: none;
    display: flex;
    align-items: center;
    gap: 6px;
    width: 100%;
    height: 26px;
    padding: 0 12px 0 6px;
    color: var(--text-1);
  }
  .sec-h:hover {
    background: var(--bg-hover);
    color: var(--text-0);
  }
  .chev {
    display: inline-flex;
    color: var(--text-2);
  }
  .num {
    font-family: var(--font-mono);
    font-size: var(--fs-xs);
  }
</style>
