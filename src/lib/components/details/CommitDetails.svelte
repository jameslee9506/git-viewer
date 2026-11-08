<!--
  One commit: message, author / committer, sha ← parents, refs, then its changed files.
  Merge commits choose the parent to diff against (P1 = details.files).
-->
<script lang="ts">
  import Archive from '@lucide/svelte/icons/archive';
  import ArrowLeft from '@lucide/svelte/icons/arrow-left';
  import Check from '@lucide/svelte/icons/check';
  import Cloud from '@lucide/svelte/icons/cloud';
  import CornerDownRight from '@lucide/svelte/icons/corner-down-right';
  import Folder from '@lucide/svelte/icons/folder';
  import FolderTree from '@lucide/svelte/icons/folder-tree';
  import GitCommitHorizontal from '@lucide/svelte/icons/git-commit-horizontal';
  import Tag from '@lucide/svelte/icons/tag';
  import { errorMessage } from '../../api';
  import { getDiffFiles } from '../../state/cache';
  import { getRepo } from '../../state/repo.svelte';
  import { settings } from '../../state/settings.svelte';
  import { toast } from '../../state/ui.svelte';
  import type { CommitDetails, DiffTarget, FileChange, Person, RefLabel } from '../../types';
  import { copyText, formatTime, shortOid, timeWithZone } from '../../util/format';
  import Spinner from '../common/Spinner.svelte';
  import Avatar from './Avatar.svelte';
  import FileList, { fileBar, fileFilter, noMatch } from './FileList.svelte';
  import type { FileListNav } from './FileList.svelte';
  import { entriesFromChanges, filterEntries, lineTotals } from './filetree';

  interface Props {
    details: CommitDetails;
  }

  let { details }: Props = $props();

  const repo = getRepo();
  const filter = fileFilter(repo);
  let list = $state.raw<FileListNav | null>();

  /** Parent chosen in the merge toggle, and the one whose files are shown (differs while loading). */
  let chosen = $state(0);
  let shown = $state(0);
  let parentFiles = $state.raw<FileChange[] | null>(null);
  let loadingParent = $state(false);
  let seq = 0;

  const files = $derived(shown === 0 || !parentFiles ? details.files : parentFiles);
  const target = $derived<DiffTarget>({
    kind: 'commit',
    oid: details.oid,
    parent: shown === 0 ? null : (details.parents[shown] ?? null),
  });
  const entries = $derived(entriesFromChanges(target, files));
  /** The entries the path filter leaves. */
  const visible = $derived(filterEntries(entries, filter.terms));
  const totals = $derived(lineTotals(files));

  const author = $derived(details.author);
  const committer = $derived(details.committer);
  const showCommitter = $derived(
    author.name !== committer.name ||
      author.email !== committer.email ||
      Math.abs(author.time - committer.time) > 60,
  );

  /** Refs of the selected graph row when it is this commit, else from the refs snapshot. */
  const refs = $derived.by((): RefLabel[] => {
    const g = repo.graph;
    const i = g.selectedRow;
    const row = i == null ? undefined : g.row(i);
    if (row && row.oid === details.oid) return row.refs;
    const r = repo.refs;
    if (!r) return [];
    const oid = details.oid;
    const out: RefLabel[] = [];
    if (r.head.detached && r.head.oid === oid) {
      out.push({ kind: 'head', name: 'HEAD', fullName: 'HEAD', current: false });
    }
    for (const b of r.locals) {
      if (b.oid === oid) out.push({ kind: 'local', name: b.name, fullName: b.fullName, current: b.isHead });
    }
    for (const b of r.remotes) {
      if (b.oid === oid) out.push({ kind: 'remote', name: b.name, fullName: b.fullName, current: false });
    }
    for (const t of r.tags) {
      if (t.oid === oid) out.push({ kind: 'tag', name: t.name, fullName: t.fullName, current: false });
    }
    return out;
  });

  async function chooseParent(k: number): Promise<void> {
    if (k === chosen) return;
    const gen = ++seq;
    chosen = k;
    if (k === 0) {
      shown = 0;
      loadingParent = false;
      return;
    }
    loadingParent = true;
    try {
      const list = await getDiffFiles(repo.id, { kind: 'commit', oid: details.oid, parent: details.parents[k] });
      if (gen !== seq) return;
      parentFiles = list;
      shown = k;
    } catch (e) {
      if (gen !== seq) return;
      chosen = shown;
      toast(errorMessage(e));
    } finally {
      if (gen === seq) loadingParent = false;
    }
  }

  function copy(text: string): void {
    void copyText(text).then(() => toast('Copied'));
  }
</script>

{#snippet person(p: Person, size: number)}
  <Avatar name={p.name} email={p.email} {size} />
  <span class="pname ellipsis" title={p.email ? `${p.name} <${p.email}>` : p.name}>{p.name}</span>
  <span class="time" title={timeWithZone(p.time, p.tz)}>{formatTime(p.time, settings.dateFormat)}</span>
{/snippet}

{#snippet parentsSeg()}
  {#if loadingParent}<Spinner size={10} />{/if}
  {#if details.parents.length > 1}
    <div class="seg">
      {#each details.parents as p, k (k)}
        <button class:on={k === chosen} title={shortOid(p)} onclick={() => void chooseParent(k)}>P{k + 1}</button>
      {/each}
    </div>
  {/if}
{/snippet}

{#snippet browse()}
  <button
    class="icon-btn"
    title="Browse files"
    onclick={() => repo.openView({ kind: 'tree', rev: details.oid, path: null })}
  >
    <FolderTree size={14} strokeWidth={1.75} />
  </button>
{/snippet}

<div class="commit">
  <div class="subject selectable">{details.subject}</div>
  {#if details.body}
    <div class="body selectable">{details.body}</div>
  {/if}

  <div class="meta">
    <div class="line">{@render person(author, 18)}</div>
    {#if showCommitter}
      <div class="line committer">
        <span class="ic" title="Committer"><CornerDownRight size={12} strokeWidth={1.75} /></span>
        {@render person(committer, 16)}
      </div>
    {/if}
    <div class="line shas">
      <span class="ic"><GitCommitHorizontal size={14} strokeWidth={1.75} /></span>
      <button class="sha" title={details.oid} onclick={() => copy(details.oid)}>{shortOid(details.oid)}</button>
      {#if details.parents.length > 0}
        <span class="ic"><ArrowLeft size={12} strokeWidth={1.75} /></span>
        {#each details.parents as p, k (k)}
          <button class="sha parent" title={p} onclick={() => void repo.reveal(p)}>{shortOid(p)}</button>
        {/each}
      {/if}
    </div>
    {#if refs.length > 0}
      <div class="refs">
        {#each refs as r, k (k)}
          <span class="badge badge-{r.kind}" class:current={r.current} title={r.fullName}>
            {#if r.current}
              <Check size={10} strokeWidth={2.5} />
            {:else if r.kind === 'remote'}
              <Cloud size={10} strokeWidth={2} />
            {:else if r.kind === 'tag'}
              <Tag size={10} strokeWidth={2} />
            {:else if r.kind === 'worktree'}
              <Folder size={10} strokeWidth={2} />
            {:else if r.kind === 'stash'}
              <Archive size={10} strokeWidth={2} />
            {/if}
            <span class="ellipsis">{r.name}</span>
          </span>
        {/each}
      </div>
    {/if}
  </div>

  {@render fileBar(
    {
      count: files.length,
      shown: visible.length,
      additions: totals.additions,
      deletions: totals.deletions,
      filter,
      list: () => list,
    },
    parentsSeg,
    browse,
  )}
  {#if visible.length > 0 || entries.length === 0}
    <FileList bind:this={list} entries={visible} filter={filter.key} />
  {:else}
    {@render noMatch()}
  {/if}
</div>

<style>
  .commit {
    flex: 1 1 0;
    min-height: 0;
    display: flex;
    flex-direction: column;
    overflow-x: hidden;
    overflow-y: auto;
  }
  .subject {
    flex: none;
    padding: 12px 12px 6px;
    color: var(--text-0);
    font-size: var(--fs);
    font-weight: 600;
    line-height: 1.35;
    overflow-wrap: anywhere;
  }
  .body {
    flex: none;
    max-height: 40%;
    overflow-y: auto;
    padding: 0 12px 8px;
    color: var(--text-1);
    font-size: var(--fs-sm);
    line-height: 1.45;
    white-space: pre-wrap;
    overflow-wrap: anywhere;
  }
  .meta {
    flex: none;
    display: flex;
    flex-direction: column;
    gap: 4px;
    padding: 4px 12px 10px;
    font-size: var(--fs-sm);
  }
  .line {
    display: flex;
    align-items: center;
    gap: 6px;
    min-width: 0;
    height: 20px;
    color: var(--text-0);
  }
  .committer {
    color: var(--text-1);
  }
  .ic {
    display: inline-flex;
    flex: none;
    color: var(--text-2);
  }
  .pname {
    flex: 0 1 auto;
  }
  .time {
    flex: none;
    margin-left: auto;
    color: var(--text-2);
    font-size: var(--fs-xs);
  }
  .shas {
    gap: 4px;
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
  .sha.parent {
    color: var(--accent);
  }
  .refs {
    display: flex;
    flex-wrap: wrap;
    gap: 4px;
    padding-top: 2px;
  }
  .refs .badge {
    max-width: 100%;
  }
</style>
