<!--
  The thin bar under the pages, for the active repo tab. Left: what is going on, as a spinner and a
  word — the running operation, the background fetch, a load that takes a while. Right (a repo with a
  remote): when its remotes were last fetched, as a button that fetches them now; a warning mark
  while the background fetch of a remote fails (git's message in the tooltip). Empty on a welcome tab.
-->
<script lang="ts">
  import CloudDownload from '@lucide/svelte/icons/cloud-download';
  import CloudOff from '@lucide/svelte/icons/cloud-off';
  import { app } from '../../state/app.svelte';
  import { OP_LABELS } from '../../state/ops.svelte';
  import { settings } from '../../state/settings.svelte';
  import { absoluteTime, relativeTime } from '../../util/format';
  import Spinner from '../common/Spinner.svelte';

  /** Loading must last this long before it is named (quick refreshes stay silent, as in the tab bar). */
  const LOADING_DELAY_MS = 300;
  /** How often the relative time is brought up to date. */
  const CLOCK_MS = 30_000;

  const repo = $derived(app.active?.repo ?? null);
  const busy = $derived(repo?.ops.busy ?? null);

  const loadingNow = $derived(!!repo && (!repo.ready || repo.graph.loading));
  let loading = $state(false);
  $effect(() => {
    if (!loadingNow) {
      loading = false;
      return;
    }
    const timer = setTimeout(() => (loading = true), LOADING_DELAY_MS);
    return () => clearTimeout(timer);
  });

  /** What the repo is doing; the user's own operation first. */
  const activity = $derived.by(() => {
    if (!repo) return null;
    if (busy) return OP_LABELS[busy];
    if (repo.autoFetch.running) return OP_LABELS.fetch;
    return loading ? 'Loading' : null;
  });

  const remote = $derived(!!repo && repo.autoFetch.has(repo));
  const fetchedAt = $derived(repo?.autoFetch.fetchedAt ?? null);
  const failures = $derived(repo ? repo.autoFetch.failures(repo) : []);
  const canFetch = $derived(remote && !busy);

  let now = $state(Date.now());
  $effect(() => {
    const tick = () => (now = Date.now());
    const timer = setInterval(tick, CLOCK_MS);
    // Timers are throttled while the window is hidden: catch up as it comes back.
    document.addEventListener('visibilitychange', tick);
    window.addEventListener('focus', tick);
    return () => {
      clearInterval(timer);
      document.removeEventListener('visibilitychange', tick);
      window.removeEventListener('focus', tick);
    };
  });

  // A fetch that just ended is `now`, whatever the clock above last read.
  const ago = $derived(fetchedAt == null ? '' : relativeTime(fetchedAt / 1000, Math.max(now, fetchedAt) / 1000));

  const title = $derived.by(() => {
    const lines = [fetchedAt == null ? 'Fetch all' : `Fetch all · fetched ${absoluteTime(fetchedAt / 1000)}`];
    if (!settings.autoFetch) lines[0] += ' · auto-fetch off';
    for (const f of failures) lines.push(`${f.remote}: ${f.error}`);
    return lines.join('\n');
  });
</script>

<!-- Clicks in the bar never take the keyboard focus from the page (graph / lists keep their keys). -->
<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
<footer class="footer" onmousedown={(e) => e.preventDefault()}>
  {#if activity}
    <span class="act" role="status">
      <Spinner size={11} />
      <span class="ellipsis">{activity}</span>
    </span>
  {/if}
  <span class="fill"></span>
  {#if repo && remote}
    <button
      class="fetch"
      class:off={!canFetch}
      class:failed={failures.length > 0}
      aria-disabled={!canFetch}
      aria-label="Fetch all"
      {title}
      onclick={() => canFetch && void repo.ops.fetch()}
    >
      {#if failures.length > 0}
        <CloudOff size={13} strokeWidth={1.75} />
      {:else}
        <CloudDownload size={13} strokeWidth={1.75} />
      {/if}
      {#if ago}<span class="ago">{ago}</span>{/if}
    </button>
  {/if}
</footer>

<style>
  .footer {
    flex: none;
    display: flex;
    align-items: center;
    gap: var(--sp-2);
    height: var(--footer-h);
    min-width: 0;
    padding: 0 var(--sp-2);
    border-top: 1px solid var(--border);
    background: var(--bg-0);
    color: var(--text-2);
    font-size: var(--fs-sm);
    white-space: nowrap;
  }

  .act {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    min-width: 0;
    padding: 0 var(--sp-1);
  }

  .fill {
    flex: 1 1 auto;
  }

  .fetch {
    flex: none;
    display: inline-flex;
    align-items: center;
    gap: 5px;
    height: 18px;
    padding: 0 6px;
    border-radius: var(--radius-sm);
    color: var(--text-2);
    transition:
      background var(--dur),
      color var(--dur);
  }
  .fetch:hover:not(.off) {
    background: var(--bg-hover);
    color: var(--text-0);
  }
  /* Not `disabled`: the tooltip (when it was fetched, what failed) stays. */
  .fetch.off {
    opacity: 0.5;
  }
  .fetch.failed :global(svg) {
    color: var(--warn);
  }
  .ago {
    font-family: var(--font-mono);
    font-size: var(--fs-xs);
    font-variant-numeric: tabular-nums;
  }
</style>
