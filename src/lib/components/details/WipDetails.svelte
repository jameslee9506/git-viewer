<!--
  Working tree of one worktree (`cwd`): HEAD bar, in-progress operation, then collapsible
  conflicted / staged / unstaged / untracked sections. `status` comes from DetailsPanel
  (repo.status for the opened worktree, a fetched status for other worktrees).
-->
<script lang="ts">
  import Check from '@lucide/svelte/icons/check';
  import Cherry from '@lucide/svelte/icons/cherry';
  import ChevronDown from '@lucide/svelte/icons/chevron-down';
  import ChevronRight from '@lucide/svelte/icons/chevron-right';
  import CircleCheck from '@lucide/svelte/icons/circle-check';
  import CircleDot from '@lucide/svelte/icons/circle-dot';
  import CirclePlus from '@lucide/svelte/icons/circle-plus';
  import Folder from '@lucide/svelte/icons/folder';
  import GitBranch from '@lucide/svelte/icons/git-branch';
  import GitCommitHorizontal from '@lucide/svelte/icons/git-commit-horizontal';
  import GitMerge from '@lucide/svelte/icons/git-merge';
  import GitPullRequestArrow from '@lucide/svelte/icons/git-pull-request-arrow';
  import Mail from '@lucide/svelte/icons/mail';
  import Split from '@lucide/svelte/icons/split';
  import TriangleExclamationPoint from '@lucide/svelte/icons/triangle-exclamation-point';
  import Undo2 from '@lucide/svelte/icons/undo-2';
  import { loadJSON, saveJSON } from '../../state/persist';
  import { getRepo } from '../../state/repo.svelte';
  import type { DiffItem } from '../../state/repo.svelte';
  import { settings } from '../../state/settings.svelte';
  import type { RepoOperation, StatusEntry, WorkingStatus } from '../../types';
  import { basename, shortOid } from '../../util/format';
  import FileList, { fileFilter, filterBox, modeSeg, noMatch } from './FileList.svelte';
  import type { FileListNav } from './FileList.svelte';
  import { displayOrder, entriesFromStatus, filterSections } from './filetree';
  import type { FileEntry } from './filetree';

  interface Props {
    cwd: string;
    status: WorkingStatus | null;
  }

  let { cwd, status }: Props = $props();

  type SectionKind = 'conflicted' | 'staged' | 'unstaged' | 'untracked';

  interface Section {
    kind: SectionKind;
    title: string;
    icon: typeof CircleDot;
    entries: FileEntry[];
  }

  const STORE = 'details.wipCollapsed';
  const KINDS: readonly SectionKind[] = ['conflicted', 'staged', 'unstaged', 'untracked'];
  const OP_ICONS: Record<RepoOperation['kind'], typeof CircleDot> = {
    merge: GitMerge,
    rebase: GitPullRequestArrow,
    'cherry-pick': Cherry,
    revert: Undo2,
    bisect: Split,
    am: Mail,
  };

  const repo = getRepo();
  const filter = fileFilter(repo);

  /** Collapsed sections (persisted, shared by all worktrees). */
  let collapsed = $state.raw<readonly SectionKind[]>(loadCollapsed());
  /** File lists of the expanded sections. */
  const lists: Partial<Record<SectionKind, FileListNav | null>> = {};

  /** Every non-empty section (none: the working tree is clean). */
  const allSections = $derived.by((): Section[] => {
    const s = status;
    if (!s) return [];
    const out: Section[] = [];
    const add = (kind: SectionKind, title: string, icon: typeof CircleDot, list: StatusEntry[]) => {
      if (list.length > 0) out.push({ kind, title, icon, entries: entriesFromStatus({ kind, cwd }, list) });
    };
    add('conflicted', 'Conflicted', TriangleExclamationPoint, s.conflicted);
    add('staged', 'Staged', CircleCheck, s.staged);
    add('unstaged', 'Unstaged', CircleDot, s.unstaged);
    add('untracked', 'Untracked', CirclePlus, s.untracked);
    return out;
  });
  /** The sections with the files the path filter leaves (empty ones dropped). */
  const sections = $derived(filterSections(allSections, filter.terms));
  /** Unfiltered size of each section: its header shows matched/total while filtering. */
  const sizes = $derived(new Map(allSections.map((s) => [s.kind, s.entries.length])));

  const other = $derived(cwd !== repo.id);

  function loadCollapsed(): SectionKind[] {
    const saved = loadJSON<unknown>(STORE, []);
    return Array.isArray(saved) ? KINDS.filter((k) => saved.includes(k)) : [];
  }

  function toggle(kind: SectionKind): void {
    collapsed = collapsed.includes(kind) ? collapsed.filter((k) => k !== kind) : [...collapsed, kind];
    saveJSON(STORE, collapsed);
  }

  /**
   * Diff siblings: every section's files in display order (so [ / ] crosses sections), and the
   * position of each entry in that list. Lazy and cached: arrow keys re-open on every step.
   */
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

  /** The first file list on screen (the filter box's ↓ / Enter). */
  function firstList(): FileListNav | null | undefined {
    const sec = sections.find((s) => !collapsed.includes(s.kind));
    return sec && lists[sec.kind];
  }
</script>

<div class="wip">
  <div class="bar">
    {#if status}
      {@const head = status.head}
      <span class="head" title={cwd}>
        {#if head.detached}
          <GitCommitHorizontal size={14} strokeWidth={1.75} />
          <span class="mono sha">{shortOid(head.oid ?? '')}</span>
        {:else}
          <GitBranch size={14} strokeWidth={1.75} />
          <span class="ellipsis">{head.branch ?? ''}</span>
        {/if}
      </span>
      {#if status.ahead > 0 || status.behind > 0}
        <span class="ab" title={status.upstream ?? ''}>
          {#if status.ahead > 0}↑{status.ahead}{/if}
          {#if status.behind > 0}↓{status.behind}{/if}
        </span>
      {/if}
      {#if other}
        <span class="wt ellipsis" title={cwd}><Folder size={12} strokeWidth={1.75} />{basename(cwd)}</span>
      {/if}
    {/if}
    {@render filterBox(filter, firstList)}
    {@render modeSeg()}
  </div>

  {#if status?.operation}
    {@const op = status.operation}
    {@const OpIcon = OP_ICONS[op.kind] ?? GitMerge}
    <div class="op" title={op.detail ?? op.kind}>
      <OpIcon size={14} strokeWidth={1.75} />
      <span class="op-kind">{op.kind}</span>
      {#if op.step != null && op.total != null}<span class="num">{op.step}/{op.total}</span>{/if}
      {#if op.detail}<span class="op-detail ellipsis">{op.detail}</span>{/if}
    </div>
  {/if}

  {#if !status}
    <div class="empty"></div>
  {:else if allSections.length === 0}
    <div class="empty"><Check size={18} strokeWidth={1.75} /></div>
  {:else if sections.length === 0}
    {@render noMatch()}
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
          <button class="sec-h {sec.kind}" title={sec.title} onclick={() => toggle(sec.kind)}>
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
  .wip {
    flex: 1 1 0;
    min-height: 0;
    display: flex;
    flex-direction: column;
  }
  .bar {
    flex: none;
    display: flex;
    align-items: center;
    gap: 8px;
    height: 30px;
    padding: 0 6px 0 12px;
    border-bottom: 1px solid var(--border);
    color: var(--text-1);
    font-size: var(--fs-sm);
    min-width: 0;
  }
  .head {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    min-width: 0;
    color: var(--text-0);
  }
  .sha {
    font-size: var(--fs-xs);
  }
  .ab {
    flex: none;
    display: inline-flex;
    gap: 4px;
    color: var(--text-2);
    font-family: var(--font-mono);
    font-size: var(--fs-xs);
  }
  .wt {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    color: var(--text-2);
    font-size: var(--fs-xs);
  }
  .op {
    flex: none;
    display: flex;
    align-items: center;
    gap: 6px;
    height: 28px;
    padding: 0 12px;
    background: var(--accent-dim);
    color: var(--text-0);
    font-size: var(--fs-sm);
    min-width: 0;
  }
  .op-kind {
    flex: none;
    font-weight: 600;
  }
  .op-detail {
    color: var(--text-1);
  }
  .num {
    flex: none;
    font-family: var(--font-mono);
    font-size: var(--fs-xs);
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
  .sec-h.conflicted {
    color: var(--st-conflict);
  }
  .chev {
    display: inline-flex;
    color: var(--text-2);
  }
</style>
