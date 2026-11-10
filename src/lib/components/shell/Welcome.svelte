<!--
  The open screen, of a new tab and of a group without tabs: a large open button, a filter over the
  recent repos (↑↓ move, Enter opens the highlighted match — or a typed absolute / ~ path when nothing
  matches, Esc clears) with a Finder button at its end, and the list. Both buttons open the Finder
  panel, in the folder of a typed path.
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
</script>

<script lang="ts">
  import { onMount } from 'svelte';
  import FolderOpen from '@lucide/svelte/icons/folder-open';
  import FolderSearch from '@lucide/svelte/icons/folder-search';
  import X from '@lucide/svelte/icons/x';
  import { errorMessage } from '../../api';
  import { app, type RecentRepo } from '../../state/app.svelte';
  import { toast } from '../../state/ui.svelte';
  import { tildify } from '../../util/format';
  import { isComposing, isEditable } from '../../util/keys';
  import Spinner from '../common/Spinner.svelte';

  interface Props {
    /** This screen is the one that shows (focuses the filter). */
    active?: boolean;
  }

  let { active = true }: Props = $props();

  let query = $state('');
  let cursor = $state(0);
  let input: HTMLInputElement | undefined = $state();
  let list: HTMLUListElement | undefined = $state();
  let home = $state<string | null>(null);

  /** Recent repos matching the query: name prefix, then name substring, then path substring. */
  const results = $derived.by(() => {
    const q = query.trim().toLowerCase();
    if (!q) return app.recent;
    const scored: [number, RecentRepo][] = [];
    for (const r of app.recent) {
      const name = r.name.toLowerCase();
      const score = name.startsWith(q)
        ? 0
        : name.includes(q)
          ? 1
          : r.path.toLowerCase().includes(q) || tildify(r.path, home).toLowerCase().includes(q)
            ? 2
            : -1;
      if (score >= 0) scored.push([score, r]);
    }
    return scored.sort((a, b) => a[0] - b[0]).map(([, r]) => r);
  });

  const current = $derived(results.length ? Math.min(cursor, results.length - 1) : -1);

  /** A path being opened that is not one of the listed repos (spinner on the open button). */
  const openingOther = $derived(app.opening !== null && !results.some((r) => r.path === app.opening));

  onMount(() => {
    void getHome().then((h) => (home = h));
  });

  $effect(() => {
    if (!active || !input) return;
    // Not out of another field the user is typing in (a group's name in the tab bar).
    const a = document.activeElement;
    if (a instanceof HTMLElement && a !== input && isEditable(a) && !a.closest('[inert]')) return;
    input.focus({ preventScroll: true });
  });

  // Keep the highlighted row visible while moving with the keyboard.
  $effect(() => {
    const i = current;
    if (i < 0 || !list) return;
    const row = list.children[i];
    if (row instanceof HTMLElement) {
      const top = row.offsetTop;
      const bottom = top + row.offsetHeight;
      if (top < list.scrollTop) list.scrollTop = top;
      else if (bottom > list.scrollTop + list.clientHeight) list.scrollTop = bottom - list.clientHeight;
    }
  });

  function open(path: string) {
    void app.open(path);
  }

  /** Finder panel, starting in the folder of a typed path. */
  function browse() {
    app.openWithDialog({ from: typedDir(query) }).catch((e) => toast(errorMessage(e)));
  }

  /** Expand a typed `~` path; null when the text is not an absolute path. */
  function typedPath(text: string): string | null {
    const t = text.trim();
    if (t.startsWith('/')) return t;
    if (home && (t === '~' || t.startsWith('~/'))) return home + t.slice(1);
    return null;
  }

  /** Folder of a typed path: up to its last `/` (what follows is a partial name), `~` as is. */
  function typedDir(text: string): string | undefined {
    const path = typedPath(text);
    if (!path) return undefined;
    return text.trim() === '~' ? path : path.slice(0, path.lastIndexOf('/') + 1);
  }

  function submit() {
    const hit = results[current];
    if (hit) {
      open(hit.path);
      return;
    }
    const path = typedPath(query);
    if (path) open(path);
  }

  function onkeydown(e: KeyboardEvent) {
    if (isComposing(e)) return;
    switch (e.code) {
      case 'ArrowDown':
        e.preventDefault();
        if (results.length) cursor = Math.min(results.length - 1, current + 1);
        break;
      case 'ArrowUp':
        e.preventDefault();
        cursor = Math.max(0, current - 1);
        break;
      case 'Enter':
      case 'NumpadEnter':
        e.preventDefault();
        submit();
        break;
      case 'Escape':
        if (query) {
          e.preventDefault();
          e.stopPropagation();
          query = '';
          cursor = 0;
        }
        break;
    }
  }
</script>

<div class="welcome">
  <div class="inner">
    <button class="open" title="Open… ⌘O" onclick={browse}>
      {#if openingOther}
        <Spinner size={22} />
      {:else}
        <FolderOpen size={28} strokeWidth={1.5} />
      {/if}
    </button>

    <div class="field">
      <input
        class="input filter"
        bind:this={input}
        bind:value={query}
        oninput={() => (cursor = 0)}
        {onkeydown}
        type="text"
        spellcheck="false"
        autocomplete="off"
        autocapitalize="off"
        aria-label="Filter"
        data-autofocus
      />
      <!-- No focus on mousedown: the caret stays in the filter. -->
      <button
        class="browse"
        tabindex="-1"
        title="Open… ⌘O"
        onmousedown={(e) => e.preventDefault()}
        onclick={browse}
      >
        <FolderSearch size={13} strokeWidth={1.75} />
      </button>
    </div>

    {#if results.length > 0}
      <ul class="list" bind:this={list}>
        {#each results as r, i (r.path)}
          <li class="item" class:current={i === current}>
            <button class="main" title={r.path} onclick={() => open(r.path)}>
              <span class="name ellipsis">{r.name}</span>
              <span class="path faint ellipsis">{tildify(r.path, home)}</span>
            </button>
            {#if app.opening === r.path}
              <span class="side"><Spinner /></span>
            {:else}
              <button class="side forget" title="Forget" onclick={() => app.forgetRecent(r.path)}>
                <X size={12} strokeWidth={1.75} />
              </button>
            {/if}
          </li>
        {/each}
      </ul>
    {/if}

    {#if app.error}
      <div class="error faint ellipsis selectable" title={app.error}>{app.error}</div>
    {/if}
  </div>
</div>

<style>
  .welcome {
    flex: 1 1 auto;
    min-height: 0;
    height: 100%;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    padding: 24px 16px 12vh;
    background: var(--bg-1);
    overflow: hidden;
  }

  .inner {
    width: min(440px, 100%);
    max-height: 100%;
    min-height: 0;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 14px;
  }

  /* A raised tile, not a bare glyph: it must read as a button. */
  .open {
    width: 64px;
    height: 64px;
    flex: none;
    display: grid;
    place-items: center;
    border: 1px solid var(--border-strong);
    border-radius: 14px;
    background: var(--bg-2);
    color: var(--text-1);
    transition:
      background var(--dur),
      color var(--dur);
  }
  .open:hover {
    background: var(--bg-3);
    color: var(--text-0);
  }

  .field {
    position: relative;
    width: 100%;
    flex: none;
    display: flex;
  }

  .filter {
    flex: 1 1 auto;
    width: 100%;
    padding-right: 28px;
  }

  .browse {
    position: absolute;
    top: 3px;
    right: 3px;
    width: 20px;
    height: 20px;
    display: grid;
    place-items: center;
    border-radius: var(--radius-sm);
    color: var(--text-2);
  }
  .browse:hover {
    background: var(--bg-hover);
    color: var(--text-0);
  }

  .list {
    position: relative;
    width: 100%;
    min-height: 0;
    margin: 0;
    padding: 0;
    list-style: none;
    overflow-y: auto;
  }

  .item {
    display: flex;
    align-items: center;
    height: 30px;
    padding-right: 4px;
    border-radius: var(--radius-sm);
  }
  .item:hover {
    background: var(--bg-hover);
  }
  .item.current {
    background: var(--bg-active);
  }

  .main {
    flex: 1 1 auto;
    min-width: 0;
    height: 100%;
    display: flex;
    align-items: baseline;
    gap: 10px;
    padding: 0 10px;
    line-height: 30px;
    text-align: left;
  }

  .name {
    flex: 0 1 auto;
    max-width: 55%;
    color: var(--text-0);
  }

  .path {
    flex: 1 1 auto;
    font-size: var(--fs-xs);
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
    background: var(--bg-hover);
  }

  .error {
    max-width: 100%;
    flex: none;
    font-size: var(--fs-sm);
  }
</style>
