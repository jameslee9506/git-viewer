<!--
  Two ends compared (from = older, to = newer): both ends, a swap button, and `git diff from to`.
  to '' is the working tree (`git diff from`, file-pen end, untracked files in their own section).
  A ⇧-range shows its oldest commit as the first end and its commit count instead of the swap button
  (a warning icon when the range is not a linear chain: then the two ends are compared).
-->
<script lang="ts">
  import ArrowUpDown from '@lucide/svelte/icons/arrow-up-down';
  import ChevronDown from '@lucide/svelte/icons/chevron-down';
  import ChevronRight from '@lucide/svelte/icons/chevron-right';
  import CircleDot from '@lucide/svelte/icons/circle-dot';
  import CirclePlus from '@lucide/svelte/icons/circle-plus';
  import FilePen from '@lucide/svelte/icons/file-pen';
  import GitCommitHorizontal from '@lucide/svelte/icons/git-commit-horizontal';
  import TriangleExclamationPoint from '@lucide/svelte/icons/triangle-exclamation-point';
  import { untrack } from 'svelte';
  import { getCommitDetails } from '../../state/cache';
  import { getRepo } from '../../state/repo.svelte';
  import type { CommitRange, DiffItem } from '../../state/repo.svelte';
  import { settings } from '../../state/settings.svelte';
  import { toast } from '../../state/ui.svelte';
  import type { DiffTarget, FileChange } from '../../types';
  import { copyText, shortOid } from '../../util/format';
  import FileList, { fileBar, fileFilter, noMatch } from './FileList.svelte';
  import type { FileListNav } from './FileList.svelte';
  import { displayOrder, entriesFromChanges, entriesFromStatus, filterSections, lineTotals } from './filetree';
  import type { FileEntry } from './filetree';

  interface Props {
    from: string;
    /** '' = the opened worktree's working tree. */
    to: string;
    fromSubject: string;
    toSubject: string;
    /** Changes from `from` to `to` (to '': to the working tree, untracked files not included). */
    files: FileChange[];
  }

  let { from, to, fromSubject, toSubject, files }: Props = $props();

  type SectionKind = 'tracked' | 'untracked';

  interface Section {
    kind: SectionKind;
    title: string;
    icon: typeof CircleDot;
    entries: FileEntry[];
  }

  const repo = getRepo();
  const filter = fileFilter(repo);

  const worktree = $derived(to === '');
  const target = $derived<DiffTarget>(
    worktree ? { kind: 'worktree', cwd: repo.id, oid: from } : { kind: 'range', from, to },
  );
  /** Untracked files of the working tree end (live from the opened worktree's status). */
  const untracked = $derived(worktree ? (repo.status?.untracked ?? []) : []);
  /** Every section; a lone one is shown without its header. */
  const allSections = $derived.by((): Section[] => {
    const out: Section[] = [];
    if (files.length > 0 || untracked.length === 0) {
      out.push({ kind: 'tracked', title: 'Changes', icon: CircleDot, entries: entriesFromChanges(target, files) });
    }
    if (untracked.length > 0) {
      out.push({
        kind: 'untracked',
        title: 'Untracked',
        icon: CirclePlus,
        entries: entriesFromStatus({ kind: 'untracked', cwd: repo.id }, untracked),
      });
    }
    return out;
  });
  /** The sections with the files the path filter leaves (empty ones dropped). */
  const sections = $derived(filterSections(allSections, filter.terms));
  /** Unfiltered size of each section: its header shows matched/total while filtering. */
  const sizes = $derived(new Map(allSections.map((s) => [s.kind, s.entries.length])));
  const shownCount = $derived(sections.reduce((n, s) => n + s.entries.length, 0));
  const totals = $derived(lineTotals(files));

  /**
   * The ⇧-range of the selection while the selection is this compare, kept once it moves on (the
   * panel shows this compare until the next one has loaded).
   */
  let lastRange: CommitRange | null = null;
  const range = $derived.by(() => {
    const s = repo.selection;
    if (s.kind === 'compare' && s.from === from && s.to === to) lastRange = s.range ?? null;
    return lastRange;
  });

  /** Subject of the range's oldest commit (`from` is its parent): its graph row (the compare row) or details. */
  let firstSubject = $state('');
  $effect(() => {
    const first = range?.first;
    if (!first) return;
    const row = untrack(() => {
      const i = repo.graph.compareRow;
      return i == null ? undefined : repo.graph.row(i);
    });
    if (row?.oid === first) {
      firstSubject = row.subject;
      return;
    }
    let live = true;
    getCommitDetails(repo.id, first).then(
      (d) => {
        if (live) firstSubject = d.subject;
      },
      () => {},
    );
    return () => {
      live = false;
    };
  });

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

  /**
   * Reverse the direction. The graph rows swap too: the store treats the selected row as `to` and the
   * compare row as `from` (re-sync after reloads, the next ⌘-click), so they must stay paired.
   * The panel keeps showing this compare while the next selection loads, so it reverses the selection
   * itself, and only while that is this compare or its reverse (a second click before the reversed
   * one has loaded): never another compare, whose rows the graph already holds, nor a ⇧-range.
   */
  function swap(): void {
    const s = repo.selection;
    if (s.kind !== 'compare' || s.range) return;
    if (!((s.from === from && s.to === to) || (s.from === to && s.to === from))) return;
    const g = repo.graph;
    [g.selectedRow, g.compareRow] = [g.compareRow, g.selectedRow];
    repo.select({ kind: 'compare', from: s.to, to: s.from });
  }

  function copy(text: string): void {
    void copyText(text).then(() => toast('Copied'));
  }
</script>

{#snippet end(oid: string, subject: string)}
  <div class="end">
    {#if oid}
      <span class="ic"><GitCommitHorizontal size={14} strokeWidth={1.75} /></span>
      <button class="sha" title={oid} onclick={() => copy(oid)}>{shortOid(oid)}</button>
      <span class="subject ellipsis selectable" title={subject}>{subject}</span>
    {:else}
      <span class="ic" title="Working tree"><FilePen size={14} strokeWidth={1.75} /></span>
    {/if}
  </div>
{/snippet}

<div class="compare">
  <div class="ends">
    <div class="pair">
      {#if range}
        {@render end(range.first, firstSubject)}
      {:else}
        {@render end(from, fromSubject)}
      {/if}
      {@render end(to, toSubject)}
    </div>
    {#if range?.count != null}
      <span class="count" title="Commits">
        <GitCommitHorizontal size={13} strokeWidth={1.75} /><span class="num">{range.count}</span>
      </span>
    {:else if range}
      <span class="warn" title="Not a linear range: the two ends are compared">
        <TriangleExclamationPoint size={14} strokeWidth={1.75} />
      </span>
    {:else if !worktree}
      <button class="icon-btn" title="Swap" onclick={swap}>
        <ArrowUpDown size={14} strokeWidth={1.75} />
      </button>
    {/if}
  </div>
  {@render fileBar({
    count: files.length + untracked.length,
    shown: shownCount,
    additions: totals.additions,
    deletions: totals.deletions,
    filter,
    list: firstList,
  })}
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
  .compare {
    flex: 1 1 0;
    min-height: 0;
    display: flex;
    flex-direction: column;
    overflow-x: hidden;
    overflow-y: auto;
  }
  .ends {
    flex: none;
    display: flex;
    align-items: center;
    gap: 4px;
    padding: 10px 6px 10px 12px;
  }
  .pair {
    flex: 1 1 auto;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .end {
    display: flex;
    align-items: center;
    gap: 4px;
    min-width: 0;
    height: 20px;
    font-size: var(--fs-sm);
  }
  .ic {
    display: inline-flex;
    flex: none;
    color: var(--text-2);
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
  .subject {
    color: var(--text-0);
  }
  .count,
  .warn {
    flex: none;
    display: inline-flex;
    align-items: center;
    gap: 3px;
    padding: 0 6px;
  }
  .count {
    color: var(--text-2);
  }
  .warn {
    color: var(--st-conflict);
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
