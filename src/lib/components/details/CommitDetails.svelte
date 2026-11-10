<!--
  One commit: a fixed-height head (DetailsHead: the sha / parents bar, the scrolling message box,
  author / committer), a summary of its changes, then its changed files. Merge commits choose
  the parent to diff against (P1 = details.files).
-->
<script lang="ts">
  import FolderTree from '@lucide/svelte/icons/folder-tree';
  import { errorMessage } from '../../api';
  import { getDiffFiles } from '../../state/cache';
  import { getRepo } from '../../state/repo.svelte';
  import { toast } from '../../state/ui.svelte';
  import type { CommitDetails, DiffTarget, FileChange, Person } from '../../types';
  import { absoluteTime, copyText, relativeTime, shortOid, timeWithZone } from '../../util/format';
  import Spinner from '../common/Spinner.svelte';
  import Avatar from './Avatar.svelte';
  import DetailsHead from './DetailsHead.svelte';
  import FileList, { changeSummary, fileBar } from './FileList.svelte';
  import { entriesFromChanges, statusCounts } from './filetree';

  interface Props {
    details: CommitDetails;
  }

  let { details }: Props = $props();

  const repo = getRepo();
  /** Height of the view: bounds the head. */
  let panelH = $state(0);

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
  const counts = $derived(statusCounts(files));

  const author = $derived(details.author);
  const committer = $derived(details.committer);
  const showCommitter = $derived(
    author.name !== committer.name ||
      author.email !== committer.email ||
      Math.abs(author.time - committer.time) > 60,
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

<div class="commit" bind:clientHeight={panelH}>
  <DetailsHead panelHeight={panelH}>
    {#snippet bar()}
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
    {/snippet}
    {#snippet message()}
      <div class="subject selectable">{details.subject}</div>
      {#if details.body}
        <div class="body selectable">{details.body}</div>
      {/if}
    {/snippet}
    {#snippet meta()}
      <div class="people">
        <Avatar name={author.name} email={author.email} size={32} />
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
              <Avatar name={committer.name} email={committer.email} size={16} />
              <span class="cname ellipsis" title={who(committer)}>{committer.name}</span>
              <span class="abs">{absoluteTime(committer.time)}</span>
            </span>
          {/if}
        </div>
      </div>
    {/snippet}
  </DetailsHead>

  {@render changeSummary(counts)}
  {@render fileBar(parentsSeg, browse)}
  <FileList {entries} />
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

  .hbar {
    flex: none;
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 2px 6px;
    min-height: var(--toolbar-h);
    padding: 4px var(--details-pad-btn) 4px var(--details-pad);
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

  /* In DetailsHead's message box (one subject line = MSG_MIN there). */
  .subject {
    color: var(--text-0);
    font-size: var(--fs-lg);
    font-weight: 500;
    line-height: 1.4;
    overflow-wrap: anywhere;
  }
  .body {
    margin-top: 8px;
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
    padding: 12px var(--details-pad);
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

  .seg > button {
    height: 22px;
    min-width: 28px;
    font-size: var(--fs-sm);
  }
</style>
