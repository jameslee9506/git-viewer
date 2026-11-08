<!--
  One repository tab: [sidebar][resizer][center][resizer][details]. The graph is always mounted; a file
  viewer (diff / blame / history / tree) covers it while repo.view is not the graph, and the graph
  layer goes visibility:hidden + inert underneath (scroll position survives).
  When the window is too narrow for the configured widths, details shrink first, then the sidebar,
  then the sidebar is left out.
-->
<script lang="ts">
  import { untrack } from 'svelte';
  import X from '@lucide/svelte/icons/x';
  import { setRepoContext, type RepoStore } from '../../state/repo.svelte';
  import { DEFAULT_SETTINGS, settings } from '../../state/settings.svelte';
  import DetailsPanel from '../details/DetailsPanel.svelte';
  import DiffView from '../diff/DiffView.svelte';
  import BlameView from '../file/BlameView.svelte';
  import FileHistoryView from '../file/FileHistoryView.svelte';
  import TreeView from '../file/TreeView.svelte';
  import GraphView from '../graph/GraphView.svelte';
  import Sidebar from '../sidebar/Sidebar.svelte';
  import Resizer from './Resizer.svelte';

  interface Props {
    repo: RepoStore;
  }

  let { repo }: Props = $props();

  // App keys this component by its store, so the context is set once.
  setRepoContext(untrack(() => repo));

  const SIDE_MIN = 180;
  const SIDE_MAX = 420;
  const DETAILS_MIN = 300;
  const DETAILS_MAX = 640;
  /** Narrowest center before the side panes give way. */
  const CENTER_MIN = 420;

  let root: HTMLDivElement | undefined = $state();
  let width = $state(0);

  function clamp(v: number, lo: number, hi: number): number {
    return Math.max(lo, Math.min(hi, Number.isFinite(v) ? v : lo));
  }

  /** Rendered pane widths for the measured width; 0 = pane not shown. */
  const layout = $derived.by(() => {
    let side = settings.sidebarVisible ? clamp(settings.sidebarWidth, SIDE_MIN, SIDE_MAX) : 0;
    const wanted = settings.detailsVisible ? clamp(settings.detailsWidth, DETAILS_MIN, DETAILS_MAX) : 0;
    let details = wanted;
    if (width > 0) {
      let over = side + details + (side ? 1 : 0) + (details ? 1 : 0) + CENTER_MIN - width;
      if (over > 0 && details) {
        const d = Math.min(over, details - DETAILS_MIN);
        details -= d;
        over -= d;
      }
      if (over > 0 && side) {
        const d = Math.min(over, side - SIDE_MIN);
        side -= d;
        over -= d;
      }
      if (over > 0 && side) {
        side = 0;
        if (details) details = clamp(width - CENTER_MIN - 1, DETAILS_MIN, wanted);
      }
    }
    return { side, details };
  });

  /** Widest each pane may be dragged without squeezing the center or the other pane. */
  const sideMax = $derived(
    width > 0
      ? clamp(width - CENTER_MIN - layout.details - (layout.details ? 2 : 1), SIDE_MIN, SIDE_MAX)
      : SIDE_MAX,
  );
  const detailsMax = $derived(
    width > 0
      ? clamp(width - CENTER_MIN - layout.side - (layout.side ? 2 : 1), DETAILS_MIN, DETAILS_MAX)
      : DETAILS_MAX,
  );

  const columns = $derived(
    [layout.side ? `${layout.side}px 1px` : '', 'minmax(0, 1fr)', layout.details ? `1px ${layout.details}px` : '']
      .filter(Boolean)
      .join(' '),
  );

  const view = $derived(repo.view);
  const viewerOpen = $derived(view.kind !== 'graph');
  const error = $derived(repo.error ?? repo.graph.error);

  function dismissError() {
    repo.error = null;
    repo.graph.error = null;
  }

  // Where focus was when a viewer opened; it goes back there when the viewer closes and took
  // the focus with it (Esc / back button inside the viewer).
  let returnFocus: HTMLElement | null = null;

  $effect.pre(() => {
    if (!viewerOpen) return;
    untrack(() => {
      const a = document.activeElement;
      returnFocus = a instanceof HTMLElement && !!root?.contains(a) ? a : null;
    });
  });

  $effect(() => {
    if (viewerOpen) return;
    const el = returnFocus;
    returnFocus = null;
    if (!el || !el.isConnected || el.closest('[inert]')) return;
    const a = document.activeElement;
    if (!a || a === document.body) el.focus({ preventScroll: true });
  });
</script>

<div class="repo" bind:this={root} bind:clientWidth={width} style:grid-template-columns={columns}>
  {#if layout.side}
    <aside class="pane side">
      <Sidebar />
    </aside>
    <Resizer
      bind:value={() => layout.side, (v) => (settings.sidebarWidth = v)}
      min={SIDE_MIN}
      max={sideMax}
      defaultValue={DEFAULT_SETTINGS.sidebarWidth}
      side="left"
    />
  {/if}

  <section class="pane center">
    {#if error}
      <div class="errbar">
        <span class="msg faint ellipsis selectable" title={error}>{error}</span>
        <button class="icon-btn" title="Dismiss" onmousedown={(e) => e.preventDefault()} onclick={dismissError}>
          <X size={12} />
        </button>
      </div>
    {/if}
    <div class="stack">
      <div class="layer" class:covered={viewerOpen} inert={viewerOpen}>
        <GraphView />
      </div>
      {#if view.kind !== 'graph'}
        <div class="layer viewer">
          {#if view.kind === 'diff'}
            <DiffView />
          {:else if view.kind === 'blame'}
            <BlameView />
          {:else if view.kind === 'history'}
            <FileHistoryView />
          {:else}
            <TreeView />
          {/if}
        </div>
      {/if}
    </div>
  </section>

  {#if layout.details}
    <Resizer
      bind:value={() => layout.details, (v) => (settings.detailsWidth = v)}
      min={DETAILS_MIN}
      max={detailsMax}
      defaultValue={DEFAULT_SETTINGS.detailsWidth}
      side="right"
    />
    <aside class="pane details">
      <DetailsPanel />
    </aside>
  {/if}
</div>

<style>
  .repo {
    flex: 1 1 auto;
    min-width: 0;
    min-height: 0;
    height: 100%;
    display: grid;
    grid-template-rows: minmax(0, 1fr);
    overflow: hidden;
  }

  .pane {
    position: relative;
    min-width: 0;
    min-height: 0;
    display: flex;
    flex-direction: column;
    overflow: hidden;
    isolation: isolate;
  }

  .side,
  .details {
    background: var(--bg-2);
  }

  .center {
    background: var(--bg-1);
  }

  .errbar {
    flex: none;
    display: flex;
    align-items: center;
    gap: 4px;
    height: 26px;
    padding: 0 4px 0 10px;
    border-bottom: 1px solid var(--border);
    background: var(--bg-2);
    font-size: var(--fs-sm);
  }
  .msg {
    flex: 1 1 auto;
  }

  .stack {
    position: relative;
    flex: 1 1 auto;
    min-height: 0;
  }

  .layer {
    position: absolute;
    inset: 0;
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }
  /* opacity also hides descendants that force `visibility: visible` */
  .layer.covered {
    visibility: hidden;
    opacity: 0;
  }

  .viewer {
    z-index: 1;
    background: var(--bg-1);
  }
</style>
