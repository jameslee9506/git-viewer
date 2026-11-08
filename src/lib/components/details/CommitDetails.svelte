<!--
  One commit: header (sha, parents), message box, author / committer, refs, a summary of its changes,
  then its changed files. Merge commits choose the parent to diff against (P1 = details.files).
-->
<script lang="ts">
  import Archive from '@lucide/svelte/icons/archive';
  import Check from '@lucide/svelte/icons/check';
  import Cloud from '@lucide/svelte/icons/cloud';
  import Folder from '@lucide/svelte/icons/folder';
  import FolderTree from '@lucide/svelte/icons/folder-tree';
  import Laptop from '@lucide/svelte/icons/laptop';
  import Tag from '@lucide/svelte/icons/tag';
  import { errorMessage } from '../../api';
  import { getDiffFiles } from '../../state/cache';
  import { getRepo } from '../../state/repo.svelte';
  import { toast } from '../../state/ui.svelte';
  import type { CommitDetails, DiffTarget, FileChange, Person, RefLabel, RefLabelKind } from '../../types';
  import { absoluteTime, copyText, relativeTime, shortOid, timeWithZone } from '../../util/format';
  import Spinner from '../common/Spinner.svelte';
  import Avatar from './Avatar.svelte';
  import FileList, { changeSummary, fileBar, fileFilter, noMatch } from './FileList.svelte';
  import type { FileListNav } from './FileList.svelte';
  import { entriesFromChanges, filterEntries, lineTotals, statusCounts } from './filetree';

  interface Props {
    details: CommitDetails;
  }

  /** The icon after a ref badge's name, as in the graph's branch / tag column (none for a detached HEAD). */
  const KIND_ICONS: Partial<Record<RefLabelKind, typeof Cloud>> = {
    local: Laptop,
    remote: Cloud,
    tag: Tag,
    stash: Archive,
    worktree: Folder,
  };

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
  const counts = $derived(statusCounts(files));

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
      out.push({ kind: 'head', name: 'HEAD', fullName: 'HEAD', current: true });
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
  /** Local branches checked out in another worktree: a folder instead of the laptop, as in the graph. */
  const elsewhere = $derived(
    new Set((repo.refs?.locals ?? []).filter((b) => b.worktree && !b.isHead).map((b) => b.fullName)),
  );

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

  function who(p: Person): string {
    return p.email ? `${p.name} <${p.email}>` : p.name;
  }
</script>

{#snippet parentsSeg()}
  {#if loadingParent}<Spinner size={12} />{/if}
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
    <FolderTree size={15} strokeWidth={1.75} />
  </button>
{/snippet}

<div class="commit">
  <div class="hbar">
    <span class="lbl">commit</span>
    <button class="sha" title={details.oid} onclick={() => copy(details.oid)}>{shortOid(details.oid)}</button>
    {#if details.parents.length > 0}
      <span class="parents">
        <span class="lbl">{details.parents.length > 1 ? 'parents' : 'parent'}</span>
        {#each details.parents as p, k (k)}
          <button class="sha link" title={p} onclick={() => void repo.reveal(p)}>{shortOid(p)}</button>
        {/each}
      </span>
    {/if}
  </div>

  <div class="msg">
    <div class="subject selectable">{details.subject}</div>
    {#if details.body}
      <div class="body selectable">{details.body}</div>
    {/if}
  </div>

  <div class="people">
    <Avatar name={author.name} email={author.email} size={36} />
    <div class="lines">
      <span class="name ellipsis" title={who(author)}>{author.name}</span>
      <span class="when" title={timeWithZone(author.time, author.tz)}>
        <span class="lbl">authored</span>
        <span class="abs">{absoluteTime(author.time)}</span>
        <span class="rel">· {relativeTime(author.time)}</span>
      </span>
      {#if showCommitter}
        <span class="when committer" title={timeWithZone(committer.time, committer.tz)}>
          <span class="lbl">committed</span>
          <Avatar name={committer.name} email={committer.email} size={18} />
          <span class="cname ellipsis" title={who(committer)}>{committer.name}</span>
          <span class="abs">{absoluteTime(committer.time)}</span>
        </span>
      {/if}
    </div>
  </div>

  {#if refs.length > 0}
    <div class="refs">
      {#each refs as r, k (k)}
        {@const Kind = r.kind === 'local' && elsewhere.has(r.fullName) ? Folder : KIND_ICONS[r.kind]}
        <span class="badge badge-{r.kind}" class:current={r.current} title={r.fullName}>
          {#if r.current}
            <Check size={12} strokeWidth={2} />
          {/if}
          <span class="ellipsis">{r.name}</span>
          {#if Kind}
            <Kind size={12} strokeWidth={2} />
          {/if}
        </span>
      {/each}
    </div>
  {/if}

  {@render changeSummary({ counts, additions: totals.additions, deletions: totals.deletions })}
  {@render fileBar({ count: files.length, shown: visible.length, filter, list: () => list }, parentsSeg, browse)}
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
    /* A size container: the body's max-height is a share of the panel height (cqh). */
    container-type: size;
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
  .parents {
    display: inline-flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 2px;
    margin-left: auto;
  }
  .parents .lbl {
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
  .sha:hover {
    background: var(--bg-hover);
  }
  .sha.link {
    color: var(--link);
  }
  .sha.link:hover {
    color: color-mix(in srgb, var(--link) 70%, var(--text-0));
  }

  .msg {
    flex: none;
    margin: 12px;
    padding: 10px 12px;
    background: var(--bg-inset);
    border: 1px solid var(--border);
    border-radius: var(--radius);
  }
  .subject {
    color: var(--text-0);
    font-size: var(--fs-lg);
    font-weight: 600;
    line-height: 1.4;
    overflow-wrap: anywhere;
  }
  .body {
    max-height: 40cqh;
    margin-top: 8px;
    overflow-y: auto;
    color: var(--text-1);
    font-size: var(--fs);
    line-height: 1.5;
    white-space: pre-wrap;
    overflow-wrap: anywhere;
  }

  .people {
    flex: none;
    display: flex;
    align-items: flex-start;
    gap: 10px;
    min-width: 0;
    padding: 0 12px 12px;
  }
  .lines {
    flex: 1 1 auto;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .name {
    color: var(--text-0);
    font-size: var(--fs);
    font-weight: 600;
    line-height: 18px;
  }
  .when {
    display: flex;
    align-items: center;
    gap: 6px;
    min-width: 0;
    font-size: var(--fs-sm);
    line-height: 18px;
    white-space: nowrap;
  }
  .abs {
    flex: none;
    color: var(--text-1);
  }
  .rel {
    flex: none;
    color: var(--text-2);
  }
  .committer {
    margin-top: 4px;
  }
  .cname {
    flex: 0 1 auto;
    color: var(--text-1);
  }

  .refs {
    flex: none;
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    padding: 0 12px 12px;
  }
  .refs .badge {
    gap: 4px;
    height: 20px;
    max-width: 100%;
    padding: 0 6px;
    border-radius: var(--radius-sm);
    font-size: var(--fs-sm);
    line-height: 20px;
  }
  .refs .badge :global(svg) {
    flex: none;
  }

  .seg > button {
    height: 22px;
    min-width: 28px;
    font-size: var(--fs-sm);
  }
</style>
