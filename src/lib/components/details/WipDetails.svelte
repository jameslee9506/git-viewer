<!--
  Working tree of one worktree (`cwd`): HEAD bar, in-progress operation, then the collapsible sections
  Conflicted (only while there are conflicts), Unstaged (tracked changes and untracked files) and Staged
  (both always shown while the tree is dirty) with stage / unstage actions on their rows and headers,
  and the commit box at the bottom (summary, description, amend, Commit). `status` comes from
  DetailsPanel (repo.status for the opened worktree, a fetched status for other worktrees).
-->
<script module lang="ts">
  import type { RepoStore } from '../../state/repo.svelte';

  /** The commit message being written in a repo tab; kept while the selection changes. */
  export class CommitDraft {
    summary = $state('');
    body = $state('');
    amend = $state(false);
  }

  const drafts = new WeakMap<RepoStore, CommitDraft>();

  /** The commit draft of a repo tab (one per tab, shared by its worktrees). */
  export function commitDraft(repo: RepoStore): CommitDraft {
    let d = drafts.get(repo);
    if (!d) {
      d = new CommitDraft();
      drafts.set(repo, d);
    }
    return d;
  }

  /** Summary lengths from which the counter shows, and from which it warns (git's conventional 72). */
  const COUNT_FROM = 50;
  const WARN_FROM = 72;
  /** The description grows with its text up to this height. */
  const DESC_MAX_H = 160;
</script>

<script lang="ts">
  import Check from '@lucide/svelte/icons/check';
  import Cherry from '@lucide/svelte/icons/cherry';
  import CircleCheck from '@lucide/svelte/icons/circle-check';
  import CircleDot from '@lucide/svelte/icons/circle-dot';
  import Folder from '@lucide/svelte/icons/folder';
  import GitBranch from '@lucide/svelte/icons/git-branch';
  import GitCommitHorizontal from '@lucide/svelte/icons/git-commit-horizontal';
  import GitMerge from '@lucide/svelte/icons/git-merge';
  import GitPullRequestArrow from '@lucide/svelte/icons/git-pull-request-arrow';
  import ListMinus from '@lucide/svelte/icons/list-minus';
  import ListPlus from '@lucide/svelte/icons/list-plus';
  import Mail from '@lucide/svelte/icons/mail';
  import Minus from '@lucide/svelte/icons/minus';
  import PencilLine from '@lucide/svelte/icons/pencil-line';
  import Plus from '@lucide/svelte/icons/plus';
  import Split from '@lucide/svelte/icons/split';
  import TriangleExclamationPoint from '@lucide/svelte/icons/triangle-exclamation-point';
  import Undo2 from '@lucide/svelte/icons/undo-2';
  import { untrack } from 'svelte';
  import { getCommitDetails } from '../../state/cache';
  import { loadJSON, saveJSON } from '../../state/persist';
  import { getRepo } from '../../state/repo.svelte';
  import type { DiffItem } from '../../state/repo.svelte';
  import { settings } from '../../state/settings.svelte';
  import type { RepoOperation, WorkingStatus } from '../../types';
  import { basename, shortOid } from '../../util/format';
  import { isComposing } from '../../util/keys';
  import Spinner from '../common/Spinner.svelte';
  import FileList, { fileFilter, filterBox, modeSeg, sectionHead, sectionMinHeight } from './FileList.svelte';
  import type { FileAction, FileListNav, SectionAction } from './FileList.svelte';
  import { displayOrder, entriesFromStatus, filterEntries } from './filetree';
  import type { FileEntry } from './filetree';

  interface Props {
    cwd: string;
    status: WorkingStatus | null;
  }

  let { cwd, status }: Props = $props();

  type SectionKind = 'conflicted' | 'unstaged' | 'staged';

  interface BaseSection {
    kind: SectionKind;
    title: string;
    icon: typeof CircleDot;
    entries: FileEntry[];
  }

  /** A section as shown: its entries after the path filter, with its row and header actions. */
  interface Section extends BaseSection {
    actions: FileAction[];
    action: SectionAction | null;
  }

  const STORE = 'details.wipCollapsed';
  const KINDS: readonly SectionKind[] = ['conflicted', 'unstaged', 'staged'];
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
  const draft = commitDraft(repo);

  /** Collapsed sections (persisted, shared by all worktrees). */
  let collapsed = $state.raw<readonly SectionKind[]>(loadCollapsed());
  /** File lists of the expanded sections. */
  const lists: Partial<Record<SectionKind, FileListNav | null>> = {};
  let descEl: HTMLTextAreaElement | undefined = $state();

  /** Nothing to stage or commit. */
  const clean = $derived.by(() => {
    const s = status;
    return !!s && s.staged.length + s.unstaged.length + s.untracked.length + s.conflicted.length === 0;
  });

  /** Conflicted (only when there are conflicts), Unstaged (changes + untracked files), Staged. */
  const allSections = $derived.by((): BaseSection[] => {
    const s = status;
    if (!s) return [];
    const out: BaseSection[] = [];
    if (s.conflicted.length > 0) {
      out.push({
        kind: 'conflicted',
        title: 'Conflicted',
        icon: TriangleExclamationPoint,
        entries: entriesFromStatus({ kind: 'conflicted', cwd }, s.conflicted),
      });
    }
    out.push({
      kind: 'unstaged',
      title: 'Unstaged',
      icon: CircleDot,
      entries: [
        ...entriesFromStatus({ kind: 'unstaged', cwd }, s.unstaged),
        ...entriesFromStatus({ kind: 'untracked', cwd }, s.untracked),
      ],
    });
    out.push({ kind: 'staged', title: 'Staged', icon: CircleCheck, entries: entriesFromStatus({ kind: 'staged', cwd }, s.staged) });
    return out;
  });
  /** The sections with the files the path filter leaves; Unstaged and Staged stay when it leaves none. */
  const sections = $derived.by((): Section[] => {
    const terms = filter.terms;
    const out: Section[] = [];
    for (const s of allSections) {
      const entries = filterEntries(s.entries, terms);
      if (entries.length === 0 && s.kind === 'conflicted') continue;
      out.push({ ...s, entries, actions: rowActions(s.kind, entries), action: headAction(s.kind, s.entries.length) });
    }
    return out;
  });
  /** Unfiltered size of each section: its header shows matched/total while filtering. */
  const sizes = $derived(new Map(allSections.map((s) => [s.kind, s.entries.length])));

  const other = $derived(cwd !== repo.id);
  const busy = $derived(repo.ops.busy);

  /** The row actions of a section over its (filtered) entries. */
  function rowActions(kind: SectionKind, entries: FileEntry[]): FileAction[] {
    const file = (i: number) => [entries[i].item.file];
    switch (kind) {
      case 'conflicted':
        return [{ icon: Check, title: 'Mark resolved', run: (i) => void repo.ops.stage(cwd, file(i)) }];
      case 'unstaged':
        return [{ icon: Plus, title: 'Stage', run: (i) => void repo.ops.stage(cwd, file(i)) }];
      case 'staged':
        return [{ icon: Minus, title: 'Unstage', run: (i) => void repo.ops.unstage(cwd, file(i)) }];
    }
  }

  /** The header action of a section (stage all / unstage all), none while it has no files. */
  function headAction(kind: SectionKind, total: number): SectionAction | null {
    if (total === 0) return null;
    if (kind === 'unstaged') return { icon: ListPlus, title: 'Stage all', run: () => void repo.ops.stageAll(cwd) };
    if (kind === 'staged') return { icon: ListMinus, title: 'Unstage all', run: () => void repo.ops.unstageAll(cwd) };
    return null;
  }

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
    const sec = sections.find((s) => !collapsed.includes(s.kind) && s.entries.length > 0);
    return sec && lists[sec.kind];
  }

  // ───────────── commit box ─────────────

  const stagedCount = $derived(status?.staged.length ?? 0);
  const canCommit = $derived.by(() => {
    const s = status;
    if (!s || busy || s.conflicted.length > 0 || draft.summary.trim() === '') return false;
    return stagedCount > 0 || (draft.amend && !s.head.unborn);
  });
  const commitTitle = $derived.by(() => {
    const s = status;
    if (!s) return '';
    if (s.conflicted.length > 0) return 'Resolve the conflicts first';
    if (stagedCount === 0 && !draft.amend) return 'Nothing staged';
    if (draft.summary.trim() === '') return 'Needs a summary';
    return draft.amend ? 'Replace the last commit (⌘↩)' : 'Commit the staged files (⌘↩)';
  });

  // The description grows with its text (also when amend fills it in).
  $effect(() => {
    void draft.body;
    void descEl;
    untrack(growDesc);
  });

  function growDesc(): void {
    const el = descEl;
    if (!el) return;
    el.style.height = 'auto';
    el.style.height = `${Math.min(DESC_MAX_H, el.scrollHeight + 2)}px`;
  }

  /** Amend on: an empty draft takes the HEAD commit's message. */
  async function toggleAmend(): Promise<void> {
    draft.amend = !draft.amend;
    const oid = status?.head.oid;
    if (!draft.amend || !oid || draft.summary.trim() || draft.body.trim()) return;
    try {
      const d = await getCommitDetails(repo.id, oid);
      if (draft.amend && !draft.summary.trim() && !draft.body.trim()) {
        draft.summary = d.subject;
        draft.body = d.body;
      }
    } catch {
      // The message stays empty; the user types one.
    }
  }

  async function commit(): Promise<void> {
    if (!canCommit) return;
    const summary = draft.summary.trim();
    const body = draft.body.trim();
    const amend = draft.amend;
    const ok = await repo.ops.commit(cwd, body ? `${summary}\n\n${body}` : summary, amend);
    // Clear the draft unless it was edited meanwhile.
    if (ok && draft.summary.trim() === summary && draft.body.trim() === body) {
      draft.summary = '';
      draft.body = '';
      draft.amend = false;
    }
  }

  /** ⌘↩ commits from either field; Esc leaves the field; Enter in the summary moves to the description. */
  function onMessageKey(e: KeyboardEvent, summary: boolean): void {
    if (isComposing(e)) return;
    const enter = e.code === 'Enter' || e.code === 'NumpadEnter';
    if (enter && e.metaKey && !e.ctrlKey && !e.altKey) {
      e.preventDefault();
      e.stopPropagation();
      void commit();
    } else if (enter && summary && !e.metaKey && !e.ctrlKey && !e.altKey && !e.shiftKey) {
      e.preventDefault();
      descEl?.focus();
    } else if (e.code === 'Escape' && !e.metaKey && !e.ctrlKey && !e.altKey && !e.shiftKey) {
      e.preventDefault();
      e.stopPropagation();
      if (e.currentTarget instanceof HTMLElement) e.currentTarget.blur();
    }
  }
</script>

<div class="wip">
  <div class="bar">
    {#if status}
      {@const head = status.head}
      <span class="head" title={cwd}>
        {#if head.detached}
          <span class="hicon"><GitCommitHorizontal size={15} strokeWidth={1.75} /></span>
          <span class="sha">{shortOid(head.oid ?? '')}</span>
        {:else}
          <span class="hicon"><GitBranch size={15} strokeWidth={1.75} /></span>
          <span class="branch ellipsis">{head.branch ?? ''}</span>
        {/if}
      </span>
      {#if status.ahead > 0 || status.behind > 0}
        <span class="ab" title={status.upstream ?? ''}>
          {#if status.ahead > 0}<span>↑{status.ahead}</span>{/if}
          {#if status.behind > 0}<span>↓{status.behind}</span>{/if}
        </span>
      {/if}
      {#if other}
        <span class="wt" title={cwd}>
          <Folder size={14} strokeWidth={1.75} /><span class="ellipsis">{basename(cwd)}</span>
        </span>
      {/if}
    {/if}
    {@render filterBox(filter, firstList)}
    {@render modeSeg()}
  </div>

  {#if status?.operation}
    {@const op = status.operation}
    {@const OpIcon = OP_ICONS[op.kind] ?? GitMerge}
    <div class="op" title={op.detail ?? op.kind}>
      <OpIcon size={16} strokeWidth={1.75} />
      <span class="op-kind">{op.kind}</span>
      {#if op.step != null && op.total != null}<span class="num">{op.step}/{op.total}</span>{/if}
      {#if op.detail}<span class="op-detail ellipsis">{op.detail}</span>{/if}
    </div>
  {/if}

  {#if !status}
    <div class="empty"></div>
  {:else if clean}
    <div class="empty"><Check size={20} strokeWidth={1.75} /></div>
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
            warn: sec.kind === 'conflicted',
            action: sec.action ?? undefined,
            ontoggle: () => toggle(sec.kind),
          })}
          {#if !closed && sec.entries.length > 0}
            <FileList
              bind:this={lists[sec.kind]}
              entries={sec.entries}
              filter={filter.key}
              actions={sec.actions}
              onopen={(i) => openFrom(sec, i)}
            />
          {/if}
        </section>
      {/each}
    </div>
  {/if}

  {#if status}
    <div class="cbox">
      <div class="summary">
        <input
          class="input cin"
          placeholder="Summary"
          spellcheck="true"
          autocomplete="off"
          aria-label="Commit summary"
          bind:value={draft.summary}
          onkeydown={(e) => onMessageKey(e, true)}
        />
        {#if draft.summary.length >= COUNT_FROM}
          <span class="clen" class:over={draft.summary.length > WARN_FROM}>{draft.summary.length}</span>
        {/if}
      </div>
      <textarea
        class="input cdesc"
        placeholder="Description"
        rows="2"
        spellcheck="true"
        aria-label="Commit description"
        bind:this={descEl}
        bind:value={draft.body}
        oninput={growDesc}
        onkeydown={(e) => onMessageKey(e, false)}
      ></textarea>
      <div class="crow">
        <button
          class="icon-btn"
          class:on={draft.amend}
          title="Amend the last commit"
          aria-pressed={draft.amend}
          disabled={status.head.unborn}
          onmousedown={(e) => e.preventDefault()}
          onclick={() => void toggleAmend()}
        >
          <PencilLine size={14} strokeWidth={1.75} />
        </button>
        <button class="cbtn" disabled={!canCommit} title={commitTitle} onclick={() => void commit()}>
          {#if busy === 'commit'}<Spinner size={12} />{/if}
          <span>{draft.amend ? 'Amend' : 'Commit'}</span>
          {#if stagedCount > 0}<span class="cn">{stagedCount}</span>{/if}
        </button>
      </div>
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
  .wip > .empty {
    flex: 1 1 0;
    min-height: 0;
    height: auto;
  }
  .bar {
    flex: none;
    display: flex;
    align-items: center;
    gap: 8px;
    min-width: 0;
    height: var(--toolbar-h);
    padding: 0 8px 0 12px;
    border-bottom: 1px solid var(--border);
    color: var(--text-1);
    font-size: var(--fs-sm);
  }
  .head {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    min-width: 0;
    color: var(--text-0);
    font-size: var(--fs);
  }
  .hicon {
    display: inline-flex;
    flex: none;
    color: var(--text-1);
  }
  .branch {
    font-weight: 600;
  }
  .sha {
    flex: none;
    font-family: var(--font-mono);
    font-size: var(--fs-code);
  }
  .ab {
    flex: none;
    display: inline-flex;
    gap: 6px;
    color: var(--text-1);
    font-family: var(--font-mono);
    font-size: var(--fs-sm);
  }
  .wt {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    min-width: 0;
    color: var(--text-2);
  }
  .wt :global(svg) {
    flex: none;
  }
  .op {
    flex: none;
    display: flex;
    align-items: center;
    gap: 8px;
    min-width: 0;
    height: 34px;
    padding: 0 12px;
    background: var(--warn-dim);
    box-shadow: inset 2px 0 0 var(--warn);
    color: var(--text-0);
    font-size: var(--fs);
  }
  .op > :global(svg) {
    color: var(--warn);
  }
  .op-kind {
    flex: none;
    font-weight: 600;
    text-transform: capitalize;
  }
  .op-detail {
    color: var(--text-1);
  }
  .num {
    flex: none;
    font-family: var(--font-mono);
    font-size: var(--fs-sm);
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

  /* commit box: summary (with a length counter from 50 characters, warning past 72), description,
     amend toggle and the Commit button with the staged count */
  .cbox {
    flex: none;
    display: flex;
    flex-direction: column;
    gap: 6px;
    padding: 8px 12px 10px;
    border-top: 1px solid var(--border);
    background: var(--bg-2);
  }
  .summary {
    position: relative;
    display: flex;
    align-items: center;
  }
  .cin {
    flex: 1 1 auto;
    width: 100%;
    height: 28px;
    padding-right: 36px;
    font-size: var(--fs);
  }
  .clen {
    position: absolute;
    right: 8px;
    color: var(--text-2);
    font-family: var(--font-mono);
    font-size: var(--fs-xs);
    pointer-events: none;
  }
  .clen.over {
    color: var(--warn);
  }
  .cdesc {
    width: 100%;
    height: auto;
    min-height: 56px;
    max-height: 160px;
    padding: 5px 8px;
    font-size: var(--fs);
    line-height: 1.4;
    resize: none;
    overflow-y: auto;
  }
  .crow {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .cbtn {
    flex: 1 1 auto;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    gap: 8px;
    height: 28px;
    border-radius: var(--radius-sm);
    background: var(--bg-3);
    color: var(--text-0);
    font-size: var(--fs);
    font-weight: 600;
    transition: background var(--dur);
  }
  .cbtn:hover:not(:disabled) {
    background: var(--bg-active-strong);
  }
  .cbtn:disabled {
    opacity: 0.4;
  }
  .cn {
    color: var(--text-1);
    font-family: var(--font-mono);
    font-size: var(--fs-sm);
    font-weight: 400;
  }
</style>
