<!--
  Working tree of one worktree (`cwd`): HEAD bar, in-progress operation (a cherry-pick or revert with
  Continue / Abort), then the collapsible sections Conflicted (only while there are conflicts), Unstaged
  (tracked changes and untracked files) and Staged (both always shown while the tree is dirty) with
  stage / unstage actions on their rows and headers, and the commit box at the bottom (the amend
  checkbox, the message — summary and description in one well — and the commit button, whose label
  says what the commit waits for). `status` comes from DetailsPanel (repo.status for the opened
  worktree, a fetched status for other worktrees).
  An open section ends in an edge, a rule that is a splitter: dragged, it sets the section's height
  (`settings.conflictedHeight` / `unstagedHeight` / `stagedHeight`), which the section then keeps
  whatever it holds; a double-click lets it follow its files again. The rules are in `sections.ts`.
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

  /** Git's conventional summary length: the counter shows what is left of it, and warns past it. */
  const SUMMARY_MAX = 72;
  /** The description grows with its text up to this height. */
  const DESC_MAX_H = 160;
  /** A section's edge: the splitter under its files, a 1px rule. */
  const EDGE_H = 1;
  /**
   * How far its edge is dragged before a section that follows its files takes a height: a press on
   * the edge, or a double-click's two, is no drag.
   */
  const EDGE_SLACK = 3;
</script>

<script lang="ts">
  import Check from '@lucide/svelte/icons/check';
  import Cherry from '@lucide/svelte/icons/cherry';
  import Folder from '@lucide/svelte/icons/folder';
  import GitBranch from '@lucide/svelte/icons/git-branch';
  import GitCommitHorizontal from '@lucide/svelte/icons/git-commit-horizontal';
  import GitMerge from '@lucide/svelte/icons/git-merge';
  import GitPullRequestArrow from '@lucide/svelte/icons/git-pull-request-arrow';
  import Mail from '@lucide/svelte/icons/mail';
  import Minus from '@lucide/svelte/icons/minus';
  import Play from '@lucide/svelte/icons/play';
  import Plus from '@lucide/svelte/icons/plus';
  import Split from '@lucide/svelte/icons/split';
  import Undo2 from '@lucide/svelte/icons/undo-2';
  import X from '@lucide/svelte/icons/x';
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
  import Resizer from '../shell/Resizer.svelte';
  import FileList, { FileSelection, ROW_H, SECTION_H, modeSeg, sectionHead } from './FileList.svelte';
  import type { FileAction, Icon, SectionAction } from './FileList.svelte';
  import { displayOrder, entriesFromStatus } from './filetree';
  import type { FileEntry } from './filetree';
  import { heldAbove, sectionHeights, sectionMax } from './sections';
  import type { SectionBox } from './sections';

  interface Props {
    cwd: string;
    status: WorkingStatus | null;
  }

  let { cwd, status }: Props = $props();

  type SectionKind = 'conflicted' | 'unstaged' | 'staged';

  interface BaseSection {
    kind: SectionKind;
    title: string;
    entries: FileEntry[];
  }

  /** A section with its row and header actions. */
  interface Section extends BaseSection {
    actions: FileAction[];
    action: SectionAction | null;
  }

  /** A section in the layout (`sections.ts`). */
  interface Box extends SectionBox {
    closed: boolean;
    /** It has an edge to drag: it is open, and has files or a set height. */
    edge: boolean;
  }

  const STORE = 'details.wipCollapsed';
  const KINDS: readonly SectionKind[] = ['conflicted', 'unstaged', 'staged'];
  /** The setting that holds a section's set height. */
  const HEIGHT = {
    conflicted: 'conflictedHeight',
    unstaged: 'unstagedHeight',
    staged: 'stagedHeight',
  } as const;
  const OP_ICONS: Record<RepoOperation['kind'], Icon> = {
    merge: GitMerge,
    rebase: GitPullRequestArrow,
    'cherry-pick': Cherry,
    revert: Undo2,
    bisect: Split,
    am: Mail,
  };

  const repo = getRepo();
  const draft = commitDraft(repo);

  /** Collapsed sections (persisted, shared by all worktrees). */
  let collapsed = $state.raw<readonly SectionKind[]>(loadCollapsed());
  /** The files picked in the sections: one selection for all of them. */
  const selection = new FileSelection();
  let descEl: HTMLTextAreaElement | undefined = $state();
  /** Height of the sections' area, and of each section in it, as laid out (0: not yet). */
  let areaH = $state(0);
  const laid = $state<Record<SectionKind, number>>({ conflicted: 0, unstaged: 0, staged: 0 });
  /** The sections' elements, and the file lists of the open ones that have files. */
  const boxEls: Partial<Record<SectionKind, HTMLElement | null>> = {};
  const lists: Partial<Record<SectionKind, ReturnType<typeof FileList> | null>> = {};

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
        entries: entriesFromStatus({ kind: 'conflicted', cwd }, s.conflicted),
      });
    }
    out.push({
      kind: 'unstaged',
      title: 'Unstaged',
      entries: [
        ...entriesFromStatus({ kind: 'unstaged', cwd }, s.unstaged),
        ...entriesFromStatus({ kind: 'untracked', cwd }, s.untracked),
      ],
    });
    out.push({ kind: 'staged', title: 'Staged', entries: entriesFromStatus({ kind: 'staged', cwd }, s.staged) });
    return out;
  });
  const sections = $derived<Section[]>(
    allSections.map((s) => ({
      ...s,
      actions: rowActions(s.kind, s.entries),
      action: headAction(s.kind, s.entries.length),
    })),
  );

  const other = $derived(cwd !== repo.id);
  const busy = $derived(repo.ops.busy);

  /** The row actions of a section over its entries: a row's file, or the files picked with it. */
  function rowActions(kind: SectionKind, entries: FileEntry[]): FileAction[] {
    const files = (ix: number[]) => ix.map((i) => entries[i].item.file);
    switch (kind) {
      case 'conflicted':
        return [{ icon: Check, title: 'Mark resolved', run: (ix) => void repo.ops.stage(cwd, files(ix)) }];
      case 'unstaged':
        return [{ icon: Plus, title: 'Stage', run: (ix) => void repo.ops.stage(cwd, files(ix)) }];
      case 'staged':
        return [{ icon: Minus, title: 'Unstage', run: (ix) => void repo.ops.unstage(cwd, files(ix)) }];
    }
  }

  /** The header button of a section (Stage All / Unstage All), none while it has no files. */
  function headAction(kind: SectionKind, total: number): SectionAction | null {
    if (total === 0) return null;
    if (kind === 'unstaged') return { label: 'Stage All', run: () => void repo.ops.stageAll(cwd) };
    if (kind === 'staged') return { label: 'Unstage All', run: () => void repo.ops.unstageAll(cwd) };
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

  // ───────────── section heights ─────────────

  /** The height a section's edge was dragged to (whole px), 0: none — also for what is no height. */
  function setHeight(kind: SectionKind): number {
    const v = settings[HEIGHT[kind]];
    return Number.isFinite(v) && v > 0 ? Math.round(v) : 0;
  }

  /**
   * The sections as the layout sees them, in order. A closed section is its header, and so is an
   * open one with neither files nor a set height (no edge: nothing is under the header); any other
   * takes at least its header, up to three rows — one, empty, when it has no files — and its edge.
   * A closed section keeps its set height for when it opens.
   */
  const boxes = $derived(
    allSections.map((s): Box => {
      const closed = collapsed.includes(s.kind);
      const set = closed ? 0 : setHeight(s.kind);
      const n = s.entries.length;
      const edge = !closed && (n > 0 || set > 0);
      return { closed, edge, set, min: SECTION_H + (edge ? Math.max(1, Math.min(n, 3)) * ROW_H + EDGE_H : 0) };
    }),
  );
  /** The height of each section that has one set, as the room allows; 0: the layout sizes it. */
  const heights = $derived(sectionHeights(areaH, boxes));
  /** Every section's height on screen: the one it is given, else the one the layout gave it. */
  const shown = $derived(allSections.map((s, i) => heights[i] || Math.max(laid[s.kind], boxes[i].min)));
  /**
   * Every section's splitter: how tall a drag makes the section at most, and its value — the
   * section's height on screen, kept within that range: heights measured in whole px may sum to a
   * pixel more than the room, and a press on an edge must not count as a drag for that.
   */
  const handles = $derived(
    allSections.map((_, i) => {
      const max = sectionMax(areaH, boxes, shown, i);
      return { max, value: Math.min(shown[i], max) };
    }),
  );

  /**
   * A drag of a section's edge: the section takes the height `px`. What is above the edge stays as
   * it is — a section there that the new height would resize is given the height it has. The
   * heights are read off the page: `shown` is a frame behind what a refresh has just changed.
   */
  function resize(kind: SectionKind, px: number): void {
    const i = allSections.findIndex((s) => s.kind === kind);
    if (i < 0) return;
    if (heights[i] === 0 && Math.abs(px - handles[i].value) < EDGE_SLACK) return;
    const now = allSections.map(
      (s, j) => heights[j] || Math.max(boxEls[s.kind]?.clientHeight ?? laid[s.kind], boxes[j].min),
    );
    const cut = (j: number) => lists[allSections[j].kind]?.overflows() ?? false;
    for (const j of heldAbove(areaH, boxes, now, cut, i, px)) settings[HEIGHT[allSections[j].kind]] = now[j];
    settings[HEIGHT[kind]] = px;
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

  // ───────────── commit box ─────────────

  const stagedCount = $derived(status?.staged.length ?? 0);
  /** Characters left of the conventional summary length; negative past it. */
  const summaryLeft = $derived(SUMMARY_MAX - draft.summary.length);
  /**
   * The commit button's label and whether nothing is missing (`ready`). The label names what the
   * commit still waits for — conflicts, then something staged (amending needs none, but a commit to
   * replace), then a summary — and, once it waits for nothing, what it does.
   */
  const commitState = $derived.by((): { label: string; ready: boolean } => {
    const s = status;
    if (!s) return { label: '', ready: false };
    const amend = draft.amend;
    if (s.conflicted.length > 0) return { label: 'Resolve conflicts to commit', ready: false };
    if (stagedCount === 0 && !(amend && !s.head.unborn)) {
      return { label: clean ? 'Nothing to commit' : 'Stage changes to commit', ready: false };
    }
    if (draft.summary.trim() === '') {
      return { label: amend ? 'Type a summary to amend' : 'Type a summary to commit', ready: false };
    }
    if (amend) return { label: 'Amend previous commit', ready: true };
    return { label: `Commit changes to ${stagedCount} ${stagedCount === 1 ? 'file' : 'files'}`, ready: true };
  });
  /** Nothing is missing and no operation is running. */
  const canCommit = $derived(commitState.ready && !busy);

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
    el.style.height = `${Math.min(DESC_MAX_H, el.scrollHeight)}px`;
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
    <div class="mode">{@render modeSeg()}</div>
  </div>

  {#if status?.operation}
    {@const op = status.operation}
    {@const OpIcon = OP_ICONS[op.kind] ?? GitMerge}
    <div class="op" title={op.detail ?? op.kind}>
      <OpIcon size={16} strokeWidth={1.75} />
      <span class="op-kind">{op.kind}</span>
      {#if op.step != null && op.total != null}<span class="num">{op.step}/{op.total}</span>{/if}
      {#if op.detail}<span class="op-detail ellipsis">{op.detail}</span>{/if}
      {#if op.kind === 'cherry-pick' || op.kind === 'revert'}
        <!-- The operations the app starts: commit what is resolved with git's prepared message and go
             on (a commit left with nothing to commit is skipped), or give up (asks first). -->
        {@const pick = op.kind}
        {@const unresolved = status.conflicted.length > 0}
        <span class="op-actions">
          <button
            class="icon-btn"
            title={unresolved ? 'Resolve the conflicts first' : 'Continue'}
            disabled={!!busy || unresolved}
            onmousedown={(e) => e.preventDefault()}
            onclick={() => void repo.ops.continuePick(cwd, pick)}
          >
            <Play size={14} strokeWidth={1.75} />
          </button>
          <button
            class="icon-btn"
            title="Abort"
            disabled={!!busy}
            onmousedown={(e) => e.preventDefault()}
            onclick={() => void repo.ops.abortPick(cwd, pick)}
          >
            <X size={14} strokeWidth={1.75} />
          </button>
        </span>
      {/if}
    </div>
  {/if}

  {#if !status}
    <div class="empty"></div>
  {:else if clean}
    <div class="empty"><Check size={20} strokeWidth={1.75} /></div>
  {:else}
    <div class="sections" bind:clientHeight={areaH}>
      {#each sections as sec, i (sec.kind)}
        {@const box = boxes[i]}
        {@const height = heights[i]}
        <!-- A set height is the section's; one that follows its files shrinks to its minimum. -->
        <section
          bind:this={() => boxEls[sec.kind], (el) => (boxEls[sec.kind] = el)}
          class="sec"
          class:closed={box.closed}
          class:sized={height > 0}
          style:height={height > 0 ? `${height}px` : null}
          style:min-height={box.closed || height > 0 ? null : `${box.min}px`}
          bind:clientHeight={laid[sec.kind]}
        >
          {@render sectionHead({
            title: sec.title,
            count: sec.entries.length,
            closed: box.closed,
            warn: sec.kind === 'conflicted',
            action: sec.action ?? undefined,
            ontoggle: () => toggle(sec.kind),
          })}
          {#if !box.closed && sec.entries.length > 0}
            <FileList
              bind:this={() => lists[sec.kind], (list) => (lists[sec.kind] = list)}
              entries={sec.entries}
              actions={sec.actions}
              {selection}
              bounded
              onopen={(k) => openFrom(sec, k)}
            />
          {/if}
          {#if box.edge}
            <!-- The edge: drag it for the section's height, double-click it for its files' again. -->
            {@const handle = handles[i]}
            <div class="edge">
              <Resizer
                bind:value={() => handle.value, (v) => resize(sec.kind, v)}
                min={box.min}
                max={handle.max}
                side="top"
                onreset={() => (settings[HEIGHT[sec.kind]] = 0)}
              />
            </div>
          {/if}
        </section>
      {/each}
    </div>
  {/if}

  {#if status}
    <div class="cbox">
      <!-- mousedown is cancelled: the keyboard focus stays in the field being typed in -->
      <button
        class="amend"
        class:on={draft.amend}
        role="checkbox"
        aria-checked={draft.amend}
        disabled={status.head.unborn}
        onmousedown={(e) => e.preventDefault()}
        onclick={() => void toggleAmend()}
      >
        <span class="box">
          {#if draft.amend}<Check size={11} strokeWidth={3} />{/if}
        </span>
        <span>Amend previous commit</span>
      </button>
      <div class="msg">
        <div class="summary">
          <input
            class="cin"
            placeholder="Summary"
            spellcheck="true"
            autocomplete="off"
            aria-label="Commit summary"
            bind:value={draft.summary}
            onkeydown={(e) => onMessageKey(e, true)}
          />
          <span class="clen" class:over={summaryLeft < 0}>{summaryLeft < 0 ? `−${-summaryLeft}` : summaryLeft}</span>
        </div>
        <textarea
          class="cdesc"
          placeholder="Description"
          rows="2"
          spellcheck="true"
          aria-label="Commit description"
          bind:this={descEl}
          bind:value={draft.body}
          oninput={growDesc}
          onkeydown={(e) => onMessageKey(e, false)}
        ></textarea>
      </div>
      <!-- The tooltip is the shortcut; what a disabled button waits for is its label. -->
      <button
        class="cbtn"
        disabled={!canCommit}
        title={canCommit ? (draft.amend ? 'Replace the last commit (⌘↩)' : 'Commit the staged files (⌘↩)') : null}
        onclick={() => void commit()}
      >
        <span class="cicon">
          {#if busy === 'commit'}
            <Spinner size={12} />
          {:else}
            <GitCommitHorizontal size={16} strokeWidth={1.75} />
          {/if}
        </span>
        <span class="ellipsis">{commitState.label}</span>
      </button>
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
    padding: 0 var(--details-pad-btn) 0 var(--details-pad);
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
  .mode {
    flex: none;
    display: flex;
    margin-left: auto;
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
    padding: 0 var(--details-pad);
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
  /* Continue / Abort at the banner's right end; a disabled one still shows its tooltip. */
  .op-actions {
    flex: none;
    display: inline-flex;
    gap: 2px;
    margin-left: auto;
  }
  .op-actions .icon-btn {
    color: var(--text-1);
  }
  /* Disabled: the tooltip still shows (it says why), the hover fill does not. */
  .op-actions .icon-btn:disabled {
    pointer-events: auto;
    background: none;
    color: var(--text-1);
  }
  .num {
    flex: none;
    font-family: var(--font-mono);
    font-size: var(--fs-sm);
  }
  /* The area's last pixel row lies on the commit box's top rule: a section that reaches the bottom
     ends in that rule — its edge is drawn over it — and not in a second one above it. */
  .sections {
    flex: 1 1 0;
    min-height: 0;
    margin-bottom: -1px;
    display: flex;
    flex-direction: column;
    overflow-y: auto;
  }
  /* Its header's rule, and under what it holds its edge (`.edge`); a closed section has the first
     alone. As tall as its files, down to its minimum when the room is short (`sectionHeights`) — or
     as tall as it was set (`.sized`). */
  .sec {
    flex: 0 1 auto;
    display: flex;
    flex-direction: column;
    min-height: 0;
  }
  .sec.closed,
  .sec.sized {
    flex: none;
  }
  /* The splitter that ends an open section, at its bottom also when its files do not fill it. */
  .edge {
    flex: none;
    margin-top: auto;
  }
  /* The last edge can lie on the area's last pixel row: its hit area ends with its rule there — the
     two pixels a splitter has under its rule would be the area's to scroll to. */
  .sec:last-child > .edge :global(.resizer)::before {
    height: 4px;
  }

  /* commit box: the amend checkbox, the message (the summary with the characters left of 72, the
     description under it) in one well, and the commit button, whose label says what it waits for */
  /* Positioned: its top rule stays over what the sections' area has in its last pixel row (it reaches
     that far, see `.sections`) — all but an edge, which is drawn over the rule. */
  .cbox {
    position: relative;
    flex: none;
    display: flex;
    flex-direction: column;
    padding: 8px var(--details-pad) 10px;
    border-top: 1px solid var(--border);
    background: var(--bg-2);
  }
  /* A checkbox drawn in ink (a native one brings the system's hue): the box, then its name. Only
     these take the click; the padding, pulled back by the margin so that the box stays on the
     panel's inset, keeps the focus ring off them. */
  .amend {
    align-self: flex-start;
    display: inline-flex;
    align-items: center;
    gap: 6px;
    height: 20px;
    margin-left: -4px;
    padding: 0 4px;
    border-radius: var(--radius-sm);
    color: var(--text-1);
    font-size: var(--fs);
    white-space: nowrap;
    transition: color var(--dur);
  }
  .amend:hover,
  .amend.on {
    color: var(--text-0);
  }
  .amend:disabled {
    opacity: 0.4;
    pointer-events: none;
  }
  .box {
    flex: none;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 14px;
    height: 14px;
    border: 1px solid var(--border-strong);
    border-radius: var(--radius-xs);
    background: var(--bg-input);
    color: var(--on-accent);
    transition:
      background var(--dur),
      border-color var(--dur);
  }
  .amend.on > .box {
    border-color: var(--accent);
    background: var(--accent);
  }
  /* One well for the whole message. Its fields are bare — no border, no background, no outline —
     and its border is their focus ring. */
  .msg {
    display: flex;
    flex-direction: column;
    margin-top: 6px;
    border: 1px solid var(--input-border);
    border-radius: var(--radius-sm);
    background: var(--bg-input);
  }
  .msg:focus-within {
    border-color: var(--focus-ring);
  }
  .summary {
    position: relative;
    display: flex;
    align-items: center;
  }
  .cin,
  .cdesc {
    width: 100%;
    min-width: 0;
    margin: 0;
    border: 0;
    background: none;
    outline: none;
  }
  .cin::placeholder,
  .cdesc::placeholder {
    color: var(--text-2);
  }
  /* Set like a commit's subject in the commit view; the counter lies over its right padding. */
  .cin {
    height: 32px;
    padding: 0 44px 0 8px;
    font-size: var(--fs-lg);
    font-weight: 500;
  }
  .cin::placeholder {
    font-weight: 400;
  }
  .clen {
    position: absolute;
    right: 8px;
    color: var(--text-2);
    font-family: var(--font-mono);
    font-size: var(--fs-sm);
    font-variant-numeric: tabular-nums;
    pointer-events: none;
  }
  .clen.over {
    color: var(--warn);
  }
  .cdesc {
    height: auto;
    min-height: 56px;
    max-height: 160px;
    padding: 2px 8px 6px;
    font-size: var(--fs);
    line-height: 1.4;
    resize: none;
    overflow-y: auto;
  }
  /* An outline while the commit waits for something — not faded: its label is to be read —, filled
     with ink once it can be made. */
  .cbtn {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 6px;
    height: 30px;
    margin-top: 8px;
    padding: 0 10px;
    border: 1px solid var(--border-strong);
    border-radius: var(--radius-sm);
    color: var(--text-2);
    font-size: var(--fs);
    font-weight: 600;
    transition:
      background var(--dur),
      border-color var(--dur),
      color var(--dur);
  }
  .cbtn:not(:disabled) {
    border-color: var(--accent);
    background: var(--accent);
    color: var(--on-accent);
  }
  .cbtn:not(:disabled):hover {
    border-color: var(--accent-hover);
    background: var(--accent-hover);
  }
  /* The fill is the focus ring's color: the ring stands off it. */
  .cbtn:focus-visible {
    outline-offset: 2px;
  }
  /* A slot as wide as the icon: the label stays put when the spinner takes the icon's place. */
  .cicon {
    flex: none;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 16px;
    height: 16px;
  }
</style>
