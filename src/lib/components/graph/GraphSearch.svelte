<!--
  Floating find box over the graph. Quick mode searches as you type; message / path / content modes
  (git-backed, slower) run on Enter. Enter / ⇧Enter step through matches, Esc closes.
-->
<script lang="ts">
  import { untrack } from 'svelte';
  import ChevronDown from '@lucide/svelte/icons/chevron-down';
  import ChevronUp from '@lucide/svelte/icons/chevron-up';
  import Code from '@lucide/svelte/icons/code';
  import FileSearch from '@lucide/svelte/icons/file-search';
  import MessageSquare from '@lucide/svelte/icons/message-square';
  import Search from '@lucide/svelte/icons/search';
  import X from '@lucide/svelte/icons/x';
  import Spinner from '../common/Spinner.svelte';
  import { getRepo } from '../../state/repo.svelte';
  import { isComposing } from '../../util/keys';
  import type { SearchMode, SearchQuery } from '../../types';

  interface Props {
    /** Query text; kept by the parent across close/open. */
    text: string;
    mode: SearchMode;
    /** Bumped by the parent to focus the input and select its text. */
    focusSeq: number;
    /** Step to the next (1) / previous (-1) match. */
    onstep: (dir: 1 | -1) => void;
    /**
     * A search just finished with `row` as its current match. `jump` is false when the user moved on
     * while it ran (another selection, a viewer opened): the match must not be selected then.
     */
    onmatch: (row: number, jump: boolean) => void;
    onclose: () => void;
  }

  let { text = $bindable(''), mode = $bindable('quick'), focusSeq, onstep, onmatch, onclose }: Props = $props();

  const repo = getRepo();
  const graph = repo.graph;
  const DEBOUNCE_MS = 120;
  const MODES: { id: SearchMode; icon: typeof Search; title: string }[] = [
    { id: 'quick', icon: Search, title: 'Subject, author, sha, refs' },
    { id: 'message', icon: MessageSquare, title: 'Message' },
    { id: 'path', icon: FileSearch, title: 'Path' },
    { id: 'content', icon: Code, title: 'Content' },
  ];

  let input: HTMLInputElement | undefined = $state();
  let timer: ReturnType<typeof setTimeout> | undefined;

  const st = $derived(graph.search);
  /** The results on screen belong to the current text + mode. */
  const fresh = $derived(!!st && st.query.text === text.trim() && st.query.mode === mode);

  $effect(() => {
    void focusSeq;
    untrack(() => {
      input?.focus();
      input?.select();
    });
  });

  $effect(() => {
    // Reopened with a quick query: show its matches again.
    untrack(() => {
      if (mode === 'quick' && text.trim() && !fresh) run();
    });
    return () => clearTimeout(timer);
  });

  function run(): void {
    clearTimeout(timer);
    timer = undefined;
    const q: SearchQuery = { text: text.trim(), mode };
    if (!q.text) {
      graph.clearSearch();
      return;
    }
    // Message / path / content searches run git and can take seconds: their match is selected only while
    // the selection is still the one they started from. Quick mode skips that check (it finishes at once,
    // and the previous keystroke's jump may still be landing); the graph must be in view in every mode.
    const sel = repo.selection;
    void graph.runSearch(q).then(() => {
      const s = graph.search;
      if (!s || s.query !== q || s.index < 0) return;
      onmatch(s.rows[s.index], repo.view.kind === 'graph' && (q.mode === 'quick' || repo.selection === sel));
    });
  }

  function oninput(): void {
    clearTimeout(timer);
    timer = undefined;
    if (!text.trim()) graph.clearSearch();
    else if (mode === 'quick') timer = setTimeout(run, DEBOUNCE_MS);
  }

  function onkeydown(e: KeyboardEvent): void {
    if (isComposing(e)) return;
    if (e.code === 'Escape') {
      e.preventDefault();
      e.stopPropagation();
      onclose();
    } else if (e.code === 'Enter' || e.code === 'NumpadEnter') {
      e.preventDefault();
      if (fresh && timer === undefined && st && !st.error) {
        if (!st.busy) onstep(e.shiftKey ? -1 : 1);
      } else run();
    }
  }

  function setMode(m: SearchMode): void {
    if (m === mode) return;
    mode = m;
    if (text.trim()) run();
    else graph.clearSearch();
  }

  /** Buttons must not take focus away from the input. */
  function keep(e: MouseEvent): void {
    e.preventDefault();
  }
</script>

<div class="gsearch" role="search">
  <label class="field">
    <Search size={13} strokeWidth={1.75} />
    <input
      bind:this={input}
      bind:value={text}
      class="input"
      type="text"
      aria-label="Find"
      spellcheck="false"
      autocomplete="off"
      autocorrect="off"
      autocapitalize="off"
      {oninput}
      {onkeydown}
    />
  </label>
  <div class="seg">
    {#each MODES as m (m.id)}
      <button class:on={mode === m.id} title={m.title} onmousedown={keep} onclick={() => setMode(m.id)}>
        <m.icon size={12} strokeWidth={1.75} />
      </button>
    {/each}
  </div>
  <span class="count mono" class:err={!!st?.error} class:stale={!!st && !fresh} title={st?.error ?? undefined}>
    {#if st?.busy}
      <Spinner size={11} />
    {:else if st}
      {st.index + 1}/{st.rows.length}
    {/if}
  </span>
  <button
    class="icon-btn"
    title="Previous ⇧⏎"
    disabled={!st || st.rows.length === 0}
    onmousedown={keep}
    onclick={() => onstep(-1)}
  >
    <ChevronUp size={14} strokeWidth={1.75} />
  </button>
  <button
    class="icon-btn"
    title="Next ⏎"
    disabled={!st || st.rows.length === 0}
    onmousedown={keep}
    onclick={() => onstep(1)}
  >
    <ChevronDown size={14} strokeWidth={1.75} />
  </button>
  <button class="icon-btn" title="Close ⎋" onmousedown={keep} onclick={onclose}>
    <X size={14} strokeWidth={1.75} />
  </button>
</div>

<style>
  .gsearch {
    position: absolute;
    top: 6px;
    right: 16px;
    z-index: 5;
    width: 320px;
    max-width: calc(100% - 28px);
    height: 28px;
    display: flex;
    align-items: center;
    gap: 2px;
    padding-right: 2px;
    background: var(--bg-3);
    border: 1px solid var(--border-strong);
    border-radius: 5px;
    box-shadow: var(--shadow);
  }
  .gsearch:focus-within {
    border-color: var(--focus-ring);
  }
  .field {
    position: relative;
    flex: 1 1 auto;
    min-width: 0;
    height: 100%;
    display: flex;
    align-items: center;
    color: var(--text-2);
  }
  .field :global(svg) {
    position: absolute;
    left: 7px;
    pointer-events: none;
  }
  .input {
    width: 100%;
    height: 100%;
    padding: 0 4px 0 25px;
    background: transparent;
    border: 0;
    border-radius: 5px;
  }
  .input:focus-visible {
    outline: none;
  }
  .seg {
    flex: none;
  }
  .seg > button {
    min-width: 22px;
    padding: 0 4px;
  }
  .count {
    flex: none;
    min-width: 34px;
    display: inline-flex;
    align-items: center;
    justify-content: flex-end;
    padding: 0 2px;
    font-size: var(--fs-xs);
    color: var(--text-2);
    font-variant-numeric: tabular-nums;
  }
  .count.stale {
    opacity: 0.5;
  }
  .count.err {
    color: var(--del-fg);
  }
  .icon-btn {
    width: 22px;
    height: 22px;
  }
</style>
