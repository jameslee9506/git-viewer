<!--
  Two ends compared (from = older, to = newer): both ends, a swap button, a summary of the changes and
  `git diff from to`. to '' is the working tree (`git diff from`, untracked files in their own section).
  A ⇧-range shows its oldest commit as the from end and its commit count instead of the swap button
  (a warning icon when the range is not a linear chain: then the two ends are compared).
-->
<script lang="ts">
  import ArrowUpDown from '@lucide/svelte/icons/arrow-up-down';
  import FilePen from '@lucide/svelte/icons/file-pen';
  import TriangleExclamationPoint from '@lucide/svelte/icons/triangle-exclamation-point';
  import { untrack } from 'svelte';
  import { getCommitDetails } from '../../state/cache';
  import { getRepo } from '../../state/repo.svelte';
  import type { CommitRange, DiffItem } from '../../state/repo.svelte';
  import { settings } from '../../state/settings.svelte';
  import { toast } from '../../state/ui.svelte';
  import type { DiffTarget, FileChange } from '../../types';
  import { copyText, shortOid } from '../../util/format';
  import FileList, { FileSelection, changeSummary, fileBar, sectionHead, sectionMinHeight } from './FileList.svelte';
  import { displayOrder, entriesFromChanges, entriesFromStatus, statusCounts } from './filetree';
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
    entries: FileEntry[];
  }

  const repo = getRepo();

  const worktree = $derived(to === '');
  const target = $derived<DiffTarget>(
    worktree ? { kind: 'worktree', cwd: repo.id, oid: from } : { kind: 'range', from, to },
  );
  /** Untracked files of the working tree end (live from the opened worktree's status). */
  const untracked = $derived(worktree ? (repo.status?.untracked ?? []) : []);
  /** Every section; a lone one is shown without its header. */
  const sections = $derived.by((): Section[] => {
    const out: Section[] = [];
    if (files.length > 0 || untracked.length === 0) {
      out.push({ kind: 'tracked', title: 'Changes', entries: entriesFromChanges(target, files) });
    }
    if (untracked.length > 0) {
      out.push({
        kind: 'untracked',
        title: 'Untracked',
        entries: entriesFromStatus({ kind: 'untracked', cwd: repo.id }, untracked),
      });
    }
    return out;
  });
  const counts = $derived(statusCounts(untracked.length > 0 ? [...files, ...untracked] : files));

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
  /** The files picked in the sections: one selection for both. */
  const selection = new FileSelection();

  function toggle(kind: SectionKind): void {
    collapsed = collapsed.includes(kind) ? collapsed.filter((k) => k !== kind) : [...collapsed, kind];
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

{#snippet end(label: string, oid: string, subject: string)}
  <div class="end">
    <span class="lbl">{label}</span>
    {#if oid}
      <button class="sha" title={oid} onclick={() => copy(oid)}>{shortOid(oid)}</button>
      <span class="subject ellipsis selectable" title={subject}>{subject}</span>
    {:else}
      <span class="wt"><FilePen size={15} strokeWidth={1.75} /></span>
      <span class="subject wt-name ellipsis">working tree</span>
    {/if}
  </div>
{/snippet}

<div class="compare">
  <div class="ends">
    <div class="pair">
      {#if range}
        {@render end('from', range.first, firstSubject)}
      {:else}
        {@render end('from', from, fromSubject)}
      {/if}
      {@render end('to', to, toSubject)}
    </div>
    {#if range?.count != null}
      <span class="count" title="Commits in the range">
        <span class="count-n">{range.count}</span>
        <span class="count-w">{range.count === 1 ? 'commit' : 'commits'}</span>
      </span>
    {:else if range}
      <span class="warn" title="Not a linear range: the two ends are compared">
        <TriangleExclamationPoint size={16} strokeWidth={1.75} />
      </span>
    {:else if !worktree}
      <button class="icon-btn swap" title="Swap" onclick={swap}>
        <ArrowUpDown size={16} strokeWidth={1.75} />
      </button>
    {/if}
  </div>
  {@render changeSummary(counts)}
  {@render fileBar()}
  {#if sections.length === 1}
    <FileList entries={sections[0].entries} {selection} />
  {:else}
    <div class="sections">
      {#each sections as sec (sec.kind)}
        {@const closed = collapsed.includes(sec.kind)}
        <section class="sec" class:closed style:min-height={closed ? null : sectionMinHeight(sec.entries.length)}>
          {@render sectionHead({
            title: sec.title,
            count: sec.entries.length,
            closed,
            ontoggle: () => toggle(sec.kind),
          })}
          {#if !closed}
            <FileList entries={sec.entries} {selection} onopen={(i) => openFrom(sec, i)} />
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
    gap: 8px;
    padding: 10px var(--details-pad-btn) 10px var(--details-pad);
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
    gap: 6px;
    min-width: 0;
    height: 24px;
    font-size: var(--fs);
  }
  /* Fixed width: the two shas line up. */
  .lbl {
    flex: none;
    width: 32px;
    color: var(--text-2);
    font-size: var(--fs-sm);
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
  .sha:hover {
    background: var(--bg-hover);
  }
  .subject {
    color: var(--text-0);
  }
  .wt {
    display: inline-flex;
    flex: none;
    padding: 0 3px;
    color: var(--text-1);
  }
  .wt-name {
    color: var(--text-1);
  }
  .count,
  .warn {
    flex: none;
    display: inline-flex;
    align-items: center;
    gap: 5px;
    padding: 0 4px;
  }
  .count {
    font-size: var(--fs);
  }
  .count-n {
    color: var(--text-0);
  }
  .count-w {
    color: var(--text-1);
  }
  .warn {
    color: var(--st-conflict);
  }
  .swap {
    width: 28px;
    height: 28px;
  }
  .sections {
    flex: 1 1 0;
    min-height: 0;
    display: flex;
    flex-direction: column;
    overflow-y: auto;
  }
  /* No rule of its own: its header's rule is the only one (`sectionHead`). */
  .sec {
    flex: 0 1 auto;
    display: flex;
    flex-direction: column;
    min-height: 0;
  }
  .sec.closed {
    flex: none;
  }
</style>
