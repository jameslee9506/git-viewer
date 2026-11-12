<!--
  The open screen, of a new tab and of a group without tabs: the heading, the Open / Clone / Create
  tiles, Clone's fields while its tile is pressed (or a clone runs for this tab), and the Recent and
  Recently Closed lists (a click opens, the hover × forgets). Open shows the Finder panel; Create
  shows it with a New Folder button and runs `git init` in the folder picked.
-->
<script module lang="ts">
  import { homeDir } from '@tauri-apps/api/path';

  let homePromise: Promise<string | null> | null = null;

  /** $HOME without a trailing slash (resolved once), null when unavailable. */
  function getHome(): Promise<string | null> {
    homePromise ??= homeDir().then(
      (h) => h.replace(/\/+$/, '') || null,
      () => null,
    );
    return homePromise;
  }

  /** The folder name git gives a clone of `url`: the last part of its path, without `.git`. */
  function repoName(url: string): string {
    const path = url.trim().replace(/\/+$/, '').replace(/\/?\.git$/, '').replace(/\/+$/, '');
    const name = path.slice(Math.max(path.lastIndexOf('/'), path.lastIndexOf(':')) + 1);
    return name === '.' || name === '..' ? '' : name;
  }

  /** `name` in the folder `dir` (as typed: `~/…` or absolute); `name` alone without a folder. */
  function join(dir: string, name: string): string {
    return dir ? `${dir.replace(/\/+$/, '')}/${name}` : name;
  }
</script>

<script lang="ts">
  import { onMount, tick, untrack } from 'svelte';
  import { open as openDialog } from '@tauri-apps/plugin-dialog';
  import CloudDownload from '@lucide/svelte/icons/cloud-download';
  import FolderOpen from '@lucide/svelte/icons/folder-open';
  import FolderSearch from '@lucide/svelte/icons/folder-search';
  import SquarePlus from '@lucide/svelte/icons/square-plus';
  import X from '@lucide/svelte/icons/x';
  import { errorMessage } from '../../api';
  import { app } from '../../state/app.svelte';
  import { toast } from '../../state/ui.svelte';
  import { basename, dirname, tildify } from '../../util/format';
  import { isComposing } from '../../util/keys';
  import Spinner from '../common/Spinner.svelte';

  interface Props {
    /** The new tab this is the page of; null on the open screen of a group without tabs. */
    tab?: number | null;
  }

  let { tab = null }: Props = $props();
  const uid = $props.id();

  let home = $state<string | null>(null);
  /** Clone's tile is pressed: its fields show. */
  let cloneOpen = $state(false);
  let url = $state('');
  /** The folder to clone into, as shown and typed: `~/…` or absolute. */
  let dest = $state('');
  /** Where the folder the URL names goes. */
  let parent = $state('');
  /** The path was typed in: the URL no longer names its folder. */
  let destTyped = $state(false);
  let urlInput: HTMLInputElement | undefined = $state();

  /** The clone running for this tab. */
  const job = $derived(app.cloneFor(tab));

  /** The folder the path names, absolute; null while it names none (a trailing `/`, no `~` yet). */
  const target = $derived.by(() => {
    const path = expand(dest);
    return path && !path.endsWith('/') ? path : null;
  });

  const ready = $derived(!job && url.trim() !== '' && target !== null);

  /** A path being opened that neither list holds (the spinner on Open). */
  const openingOther = $derived(
    app.opening !== null &&
      app.opening !== app.creating &&
      !app.recent.some((r) => r.path === app.opening) &&
      !app.closed.some((r) => r.path === app.opening),
  );

  onMount(() => {
    void getHome().then((h) => (home = h));
  });

  $effect(() => {
    const j = job;
    if (!j) return;
    untrack(() => {
      cloneOpen = true;
      // Begun on the screen of a group without tabs, it runs in a new tab: its fields come along.
      if (url || dest) return;
      url = j.url;
      dest = tildify(j.dest, home);
      destTyped = true;
    });
  });

  /** A typed path made absolute (`~` → $HOME); null when it is not one. */
  function expand(text: string): string | null {
    const t = text.trim();
    if (t.startsWith('/')) return t;
    if (home && (t === '~' || t.startsWith('~/'))) return home + t.slice(1);
    return null;
  }

  /** Next to the last opened repository, else in the home folder. */
  function defaultParent(): string {
    const last = app.recent[0]?.path;
    if (last) return tildify(dirname(last) || '/', home);
    return home ? '~' : '';
  }

  function open(path: string) {
    void app.open(path);
  }

  function browse() {
    app.openWithDialog().catch((e) => toast(errorMessage(e)));
  }

  function create() {
    app.createWithDialog().catch((e) => toast(errorMessage(e)));
  }

  async function toggleClone() {
    if (job) return;
    cloneOpen = !cloneOpen;
    if (!cloneOpen) return;
    if (!destTyped) {
      parent ||= defaultParent();
      dest = join(parent, repoName(url));
    }
    await tick();
    urlInput?.focus();
  }

  function onUrlInput() {
    if (!destTyped) dest = join(parent, repoName(url));
  }

  function onDestInput() {
    destTyped = dest.trim() !== '';
  }

  /** Finder panel for the folder the clone goes in; the path keeps its name. */
  async function chooseFolder() {
    const from = expand(dest);
    let picked: string | null;
    try {
      picked = await openDialog({
        directory: true,
        multiple: false,
        canCreateDirectories: true,
        defaultPath: from ? dirname(from) || '/' : undefined,
      });
    } catch (e) {
      toast(errorMessage(e));
      return;
    }
    if (!picked) return;
    parent = tildify(picked, home);
    dest = join(parent, (destTyped && basename(dest.trim())) || repoName(url));
  }

  function submit() {
    if (ready && target) void app.clone(url.trim(), target, tab);
  }

  function cancel() {
    if (job) app.cancelClone(job.tab);
  }

  /**
   * A click handler deaf to the second click of a double-click: as a clone starts or stops, Clone and
   * Cancel take each other's place under the pointer, and Clone's fields come and go with its tile.
   */
  function single(run: () => void): (e: MouseEvent) => void {
    return (e) => {
      if (e.detail < 2) run();
    };
  }

  function onFieldKey(e: KeyboardEvent) {
    if (isComposing(e)) return;
    switch (e.code) {
      case 'Enter':
      case 'NumpadEnter':
        e.preventDefault();
        submit();
        break;
      case 'Escape':
        if (job) break;
        e.preventDefault();
        e.stopPropagation();
        cloneOpen = false;
        break;
    }
  }
</script>

{#snippet row(r: { path: string; name: string }, forget: () => void)}
  <li class="item">
    <button class="main" title={r.path} onclick={() => open(r.path)}>
      <span class="repo ellipsis">{r.name}</span>
      <span class="path ellipsis">{tildify(r.path, home)}</span>
    </button>
    {#if app.opening === r.path}
      <span class="side"><Spinner /></span>
    {:else}
      <button class="side forget" title="Forget" onclick={forget}>
        <X size={12} strokeWidth={1.75} />
      </button>
    {/if}
  </li>
{/snippet}

<div class="welcome">
  <div class="inner">
    <h1 class="title">Repositories</h1>

    <div class="actions">
      <button class="action" title="Open… ⌘O" onclick={browse}>
        <span class="ico">
          {#if openingOther}<Spinner size={16} />{:else}<FolderOpen size={18} />{/if}
        </span>
        Open
      </button>
      <button
        class="action"
        class:on={cloneOpen || job}
        aria-pressed={cloneOpen || job !== null}
        onclick={single(toggleClone)}
      >
        <span class="ico"><CloudDownload size={18} /></span>
        Clone
      </button>
      <button class="action" onclick={create}>
        <span class="ico">
          {#if app.creating}<Spinner size={16} />{:else}<SquarePlus size={18} />{/if}
        </span>
        Create
      </button>
    </div>

    {#if cloneOpen || job}
      <div class="clone">
        <label class="field" for="{uid}-url">URL</label>
        <input
          id="{uid}-url"
          class="input url"
          bind:this={urlInput}
          bind:value={url}
          oninput={onUrlInput}
          onkeydown={onFieldKey}
          disabled={job !== null}
          type="text"
          spellcheck="false"
          autocomplete="off"
          autocapitalize="off"
          autocorrect="off"
          data-autofocus
        />
        <label class="field" for="{uid}-dest">Path</label>
        <span class="dest">
          <input
            id="{uid}-dest"
            class="input"
            bind:value={dest}
            oninput={onDestInput}
            onkeydown={onFieldKey}
            disabled={job !== null}
            title={target ?? undefined}
            type="text"
            spellcheck="false"
            autocomplete="off"
            autocapitalize="off"
            autocorrect="off"
          />
          <button class="icon-btn browse" title="Choose…" disabled={job !== null} onclick={chooseFolder}>
            <FolderSearch size={13} />
          </button>
        </span>
        {#if job}
          <span class="run">
            <Spinner />
            <button class="btn" onclick={single(cancel)}>Cancel</button>
          </span>
        {:else}
          <button class="btn go" disabled={!ready} onclick={single(submit)}>Clone</button>
        {/if}
      </div>
    {/if}

    {#if app.error}
      <div class="error faint selectable">{app.error}</div>
    {/if}

    {#if app.recent.length > 0}
      <section class="section">
        <h2 class="heading">Recent</h2>
        <ul class="list">
          {#each app.recent as r (r.path)}
            {@render row(r, () => app.forgetRecent(r.path))}
          {/each}
        </ul>
      </section>
    {/if}

    {#if app.closed.length > 0}
      <section class="section">
        <h2 class="heading">Recently Closed</h2>
        <ul class="list">
          {#each app.closed as r (r.path)}
            {@render row(r, () => app.forgetClosed(r.path))}
          {/each}
        </ul>
      </section>
    {/if}
  </div>
</div>

<style>
  .welcome {
    flex: 1 1 auto;
    min-height: 0;
    height: 100%;
    overflow-y: auto;
    padding: max(24px, 10vh) 24px 32px;
    background: var(--bg-1);
  }

  .inner {
    width: min(560px, 100%);
    margin: 0 auto;
    display: flex;
    flex-direction: column;
  }

  .title {
    margin: 0 0 16px;
    font-size: var(--fs-title);
    font-weight: 400;
    line-height: 1.3;
    color: var(--text-1);
  }

  .actions {
    display: flex;
    flex-wrap: wrap;
    gap: 10px;
  }

  /* Raised tiles with their names. */
  .action {
    height: var(--action-h);
    display: inline-flex;
    align-items: center;
    gap: 9px;
    padding: 0 16px 0 13px;
    border: 1px solid var(--border-strong);
    border-radius: var(--radius-sm);
    background: var(--bg-2);
    color: var(--text-0);
    font-size: var(--fs-lg);
    font-weight: 500;
    transition: background var(--dur);
  }
  .action:hover {
    background: var(--bg-3);
  }
  .action.on {
    background: var(--accent-dim);
  }

  .ico {
    width: 18px;
    height: 18px;
    flex: none;
    display: grid;
    place-items: center;
    color: var(--text-1);
    transition: color var(--dur);
  }
  .action:hover .ico,
  .action.on .ico {
    color: var(--text-0);
  }

  /* URL, path, button: two rows, the field names in a column of their own. */
  .clone {
    display: grid;
    grid-template-columns: max-content minmax(0, 1fr) max-content;
    align-items: center;
    gap: 8px 10px;
    margin-top: 14px;
  }

  .field {
    font-size: var(--fs-sm);
    color: var(--text-2);
  }

  .clone .input {
    height: 28px;
    font-size: var(--fs);
  }
  .clone .input:disabled {
    color: var(--text-1);
  }

  .url {
    grid-column: 2 / 4;
  }

  .dest {
    position: relative;
    min-width: 0;
    display: flex;
  }
  .dest .input {
    flex: 1 1 auto;
    width: 100%;
    padding-right: 30px;
  }

  .browse {
    position: absolute;
    top: 2px;
    right: 2px;
  }

  .run {
    display: inline-flex;
    align-items: center;
    gap: 8px;
  }

  .btn {
    height: 28px;
    padding: 0 12px;
    border: 1px solid var(--border-strong);
    border-radius: var(--radius-sm);
    color: var(--text-1);
    font-size: var(--fs-sm);
    font-weight: 600;
    transition:
      background var(--dur),
      color var(--dur);
  }
  .btn:hover {
    background: var(--bg-3);
    color: var(--text-0);
  }
  .btn:disabled {
    opacity: 0.4;
    pointer-events: none;
  }
  /* Ink once it can clone, like the commit button once it can commit. */
  .go:not(:disabled) {
    border-color: var(--accent);
    background: var(--accent);
    color: var(--on-accent);
  }
  .go:not(:disabled):hover {
    border-color: var(--accent-hover);
    background: var(--accent-hover);
  }

  .error {
    margin-top: 12px;
    font-size: var(--fs-sm);
    white-space: pre-line;
    overflow-wrap: anywhere;
  }

  .section {
    margin-top: 28px;
  }
  .section + .section {
    margin-top: 24px;
  }

  .heading {
    margin: 0 0 4px;
    font-size: var(--fs-repo);
    font-weight: 400;
    color: var(--text-1);
  }

  .list {
    margin: 0;
    padding: 0;
    list-style: none;
  }

  /* The hover band reaches a little past the text, which sits just inside the section's name. */
  .item {
    display: flex;
    align-items: center;
    height: var(--list-h);
    margin: 0 -3px;
    padding-right: 2px;
    border-radius: var(--radius-sm);
  }
  .item:hover {
    background: var(--bg-hover);
  }

  .main {
    flex: 1 1 auto;
    min-width: 0;
    height: 100%;
    display: flex;
    align-items: baseline;
    gap: 12px;
    padding: 0 6px;
    font-size: var(--fs-repo);
    line-height: var(--list-h);
    text-align: left;
  }

  .repo {
    flex: 0 1 auto;
    max-width: 60%;
    color: var(--link);
    font-weight: 600;
  }

  .path {
    flex: 1 1 auto;
    color: var(--text-2);
  }

  .side {
    width: 20px;
    height: 20px;
    flex: none;
    display: grid;
    place-items: center;
    border-radius: var(--radius-sm);
  }

  .forget {
    color: var(--text-2);
    visibility: hidden;
  }
  .item:hover .forget {
    visibility: visible;
  }
  .forget:hover {
    color: var(--text-0);
    background: var(--bg-3);
  }
</style>
