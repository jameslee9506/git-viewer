<!--
  Right-hand panel for repo.selection. Loads everything a view needs before switching to it, so the
  previous content stays until the new data arrives (thin progress line meanwhile). Selection bursts
  (arrow-key scrolling) are debounced; stale responses are dropped by a generation counter.
-->
<script lang="ts">
  import TriangleExclamationPoint from '@lucide/svelte/icons/triangle-exclamation-point';
  import { untrack } from 'svelte';
  import { api, errorMessage } from '../../api';
  import { getCommitDetails, getDiffFiles } from '../../state/cache';
  import { getRepo } from '../../state/repo.svelte';
  import type { Selection } from '../../state/repo.svelte';
  import type { CommitDetails as Details, DiffTarget, FileChange, Stash, WorkingStatus } from '../../types';
  import CommitDetails from './CommitDetails.svelte';
  import CompareDetails from './CompareDetails.svelte';
  import StashDetails from './StashDetails.svelte';
  import WipDetails from './WipDetails.svelte';

  type Model =
    | { kind: 'none' }
    | { kind: 'error'; message: string }
    | { kind: 'commit'; details: Details }
    /** status null = the opened worktree (live repo.status). */
    | { kind: 'wip'; cwd: string; status: WorkingStatus | null }
    | { kind: 'stash'; oid: string; stash: Stash | null; files: FileChange[]; untracked: FileChange[] | null }
    | { kind: 'compare'; from: string; to: string; fromSubject: string; toSubject: string; files: FileChange[] };

  /** Trailing delay while selection changes come in bursts. */
  const DEBOUNCE_MS = 60;
  /** A change arriving later than this after the previous one loads immediately. */
  const BURST_MS = 150;

  const repo = getRepo();

  let model = $state.raw<Model>({ kind: 'none' });
  let loading = $state(false);

  let seq = 0;
  let timer: ReturnType<typeof setTimeout> | undefined;
  let lastChange = Number.NEGATIVE_INFINITY;

  /** What the panel shows; other worktrees' status is also refetched when the working tree changes. */
  const loadKey = $derived.by(() => {
    const s = repo.selection;
    switch (s.kind) {
      case 'commit':
        return `commit\0${s.oid}`;
      case 'stash':
        return `stash\0${s.oid}`;
      case 'compare':
        // A working-tree end ('') is live: refetch when the worktree changes. A ⇧-range loads no `from`
        // subject (see load): a plain compare of the same two ends is another load.
        return s.to === ''
          ? `compare\0${s.from}\0\0${repo.worktreeVersion}`
          : `compare\0${s.from}\0${s.to}${s.range ? '\0range' : ''}`;
      case 'wip':
        return s.cwd === repo.id ? `wip\0${s.cwd}` : `wip\0${s.cwd}\0${repo.worktreeVersion}`;
      default:
        return 'none';
    }
  });

  $effect(() => {
    void loadKey;
    untrack(() => schedule(repo.selection));
    return () => {
      clearTimeout(timer);
      seq++;
    };
  });

  function schedule(sel: Selection): void {
    clearTimeout(timer);
    const gen = ++seq;
    if (sel.kind === 'none') {
      model = { kind: 'none' };
      loading = false;
      return;
    }
    loading = true;
    const now = performance.now();
    const burst = now - lastChange < BURST_MS;
    lastChange = now;
    if (burst) timer = setTimeout(() => void run(sel, gen), DEBOUNCE_MS);
    else void run(sel, gen);
  }

  async function run(sel: Selection, gen: number): Promise<void> {
    try {
      const next = await load(sel);
      if (gen === seq) model = next;
    } catch (e) {
      if (gen === seq) model = { kind: 'error', message: errorMessage(e) };
    } finally {
      if (gen === seq) loading = false;
    }
  }

  async function load(sel: Selection): Promise<Model> {
    const id = repo.id;
    switch (sel.kind) {
      case 'commit':
        return { kind: 'commit', details: await getCommitDetails(id, sel.oid) };
      case 'wip':
        return { kind: 'wip', cwd: sel.cwd, status: sel.cwd === id ? null : await api.status(sel.cwd) };
      case 'stash': {
        const oid = sel.oid;
        const stash = repo.refs?.stashes.find((s) => s.oid === oid) ?? null;
        const [files, untracked] = await Promise.all([
          getDiffFiles(id, { kind: 'stash', oid }),
          // Unknown stash (refs not loaded yet): try the third parent, absent → none.
          stash && !stash.untrackedCommit
            ? null
            : getDiffFiles(id, { kind: 'stashUntracked', oid }).catch(() => null),
        ]);
        return { kind: 'stash', oid, stash, files, untracked };
      }
      case 'compare': {
        const { from, to } = sel;
        const target: DiffTarget = to === '' ? { kind: 'worktree', cwd: id, oid: from } : { kind: 'range', from, to };
        const [files, fromSubject, toSubject] = await Promise.all([
          getDiffFiles(id, target),
          // A ⇧-range shows its oldest commit (CompareDetails reads that subject), never `from`. For a
          // linear range `from` is that commit's parent (or the empty tree), no row of the selection:
          // its subject would cost a commit_details run (a diff of that commit) the panel waits for.
          sel.range ? '' : subjectOf(from),
          to === '' ? '' : subjectOf(to),
        ]);
        return { kind: 'compare', from, to, fromSubject, toSubject, files };
      }
      default:
        return { kind: 'none' };
    }
  }

  /** Subject from the selected/compared graph rows when loaded, else from the commit details. */
  async function subjectOf(oid: string): Promise<string> {
    const g = repo.graph;
    for (const i of [g.selectedRow, g.compareRow]) {
      const row = i == null ? undefined : g.row(i);
      if (row && row.oid === oid) return row.subject;
    }
    try {
      return (await getCommitDetails(repo.id, oid)).subject;
    } catch {
      return '';
    }
  }
</script>

<div class="details">
  {#if loading}<div class="progress"></div>{/if}
  {#if model.kind === 'commit'}
    {#key model.details.oid}
      <CommitDetails details={model.details} />
    {/key}
  {:else if model.kind === 'wip'}
    {#key model.cwd}
      <WipDetails cwd={model.cwd} status={model.cwd === repo.id ? repo.status : model.status} />
    {/key}
  {:else if model.kind === 'stash'}
    {#key model.oid}
      <StashDetails oid={model.oid} stash={model.stash} files={model.files} untracked={model.untracked} />
    {/key}
  {:else if model.kind === 'compare'}
    {#key `${model.from}\0${model.to}`}
      <CompareDetails
        from={model.from}
        to={model.to}
        fromSubject={model.fromSubject}
        toSubject={model.toSubject}
        files={model.files}
      />
    {/key}
  {:else if model.kind === 'error'}
    <div class="empty" title={model.message}><TriangleExclamationPoint size={16} strokeWidth={1.75} /></div>
  {:else}
    <div class="empty"></div>
  {/if}
</div>

<style>
  .details {
    position: relative;
    display: flex;
    flex-direction: column;
    height: 100%;
    min-height: 0;
    min-width: 0;
    overflow: hidden;
    background: var(--bg-2);
  }
  .details > .empty {
    flex: 1 1 auto;
  }
  .progress {
    position: absolute;
    top: 0;
    left: 0;
    right: 0;
    z-index: 2;
    height: 1px;
    overflow: hidden;
    opacity: 0;
    pointer-events: none;
    animation: progress-in 120ms ease-out 150ms forwards;
  }
  .progress::after {
    content: '';
    position: absolute;
    top: 0;
    bottom: 0;
    left: 0;
    width: 30%;
    background: var(--accent);
    animation: progress-slide 1s ease-in-out infinite;
  }
  @keyframes progress-in {
    to {
      opacity: 1;
    }
  }
  @keyframes progress-slide {
    from {
      transform: translateX(-100%);
    }
    to {
      transform: translateX(340%);
    }
  }
</style>
