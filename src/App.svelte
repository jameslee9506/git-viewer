<!--
  App shell: tab bar, one stacked page per tab (inactive pages stay mounted: visibility hidden + inert,
  moved out of the window), the footer, the global context menu, ref popover and toast. Wires native
  menu events, Dock/CLI opens, folder drops, global keys, window focus (user.css reload + refresh), the
  background fetch of the active tab, webview zoom and graph reloads on graph settings.
-->
<script lang="ts">
  import { onMount, untrack } from 'svelte';
  import { setLucideProps } from '@lucide/svelte/context';
  import { getCurrentWebview, type DragDropEvent } from '@tauri-apps/api/webview';
  import { getCurrentWindow } from '@tauri-apps/api/window';
  import ContextMenu from './lib/components/common/ContextMenu.svelte';
  import Toast from './lib/components/common/Toast.svelte';
  import RefPopover from './lib/components/graph/RefPopover.svelte';
  import Footer from './lib/components/shell/Footer.svelte';
  import RepoView from './lib/components/shell/RepoView.svelte';
  import { clampZoom, stepZoom } from './lib/components/shell/SettingsPopover.svelte';
  import TabBar from './lib/components/shell/TabBar.svelte';
  import Welcome from './lib/components/shell/Welcome.svelte';
  import { api, errorMessage, onMenu, onOpenPaths } from './lib/api';
  import { app } from './lib/state/app.svelte';
  import { startAutoFetch } from './lib/state/autofetch.svelte';
  import type { CenterView, RepoStore, Selection } from './lib/state/repo.svelte';
  import { settings } from './lib/state/settings.svelte';
  import { refreshTheme } from './lib/state/theme.svelte';
  import { hideContextMenu, toast } from './lib/state/ui.svelte';
  import { updater } from './lib/state/updater.svelte';
  import type { MenuId } from './lib/types';
  import { isComposing, isEditable, matches } from './lib/util/keys';

  setLucideProps({ size: 14, strokeWidth: 1.75 });

  /** Minimum gap between focus-triggered refreshes. */
  const FOCUS_REFRESH_MS = 2000;
  /** Debounce before graph-affecting settings reload every open graph. */
  const GRAPH_RELOAD_MS = 200;

  let settingsOpen = $state(false);
  /** Files are being dragged over the window. */
  let dropping = $state(false);

  /** Start an async Tauri subscription; returns a sync disposer (safe if it resolves after disposal). */
  function subscribe(start: () => Promise<() => void>): () => void {
    let unlisten: (() => void) | undefined;
    let disposed = false;
    try {
      start().then(
        (u) => (disposed ? u() : (unlisten = u)),
        () => {},
      );
    } catch {
      /* not running inside Tauri */
    }
    return () => {
      disposed = true;
      unlisten?.();
    };
  }

  onMount(() => {
    const stops = [
      updater.start(),
      startAutoFetch(() => app.active?.repo ?? null),
      subscribe(() => onMenu((e) => onMenuEvent(e.id))),
      subscribe(() => onOpenPaths((paths) => void openPaths(paths))),
      subscribe(() => getCurrentWebview().onDragDropEvent((e) => onDragDrop(e.payload))),
      subscribe(() => getCurrentWindow().onFocusChanged((e) => e.payload && onWindowFocus())),
    ];
    // Stylesheets of lazily loaded chunks land in <head> later: keep user.css after them.
    const headObserver = new MutationObserver(() => {
      const el = document.getElementById('user-css');
      if (el && document.head.lastElementChild !== el) document.head.appendChild(el);
    });
    headObserver.observe(document.head, { childList: true });
    void applyUserCss();
    void app.restore();
    return () => {
      headObserver.disconnect();
      for (const stop of stops) stop();
    };
  });

  // ───────────── actions ─────────────

  function openDialog() {
    app.openWithDialog().catch((e) => toast(errorMessage(e)));
  }

  /** Open each path in a new tab (an active empty tab is used first). Sequential: keeps tab order. */
  async function openPaths(paths: string[]) {
    for (const path of paths) await app.open(path, { newTab: app.active?.repo != null });
  }

  /**
   * Find in graph: the search box lives on the graph, so leave any viewer first. ⌘F comes as a menu
   * event, which closes no context menu: one left open would keep every key from the search box.
   */
  function find() {
    const repo = app.active?.repo;
    if (!repo) return;
    hideContextMenu();
    if (repo.view.kind !== 'graph') repo.closeView();
    repo.findRequest++;
  }

  function zoom(dir: 1 | -1 | 0) {
    toast(`${Math.round(stepZoom(dir) * 100)}%`);
  }

  function onMenuEvent(id: MenuId) {
    const repo = app.active?.repo ?? null;
    switch (id) {
      case 'open':
        openDialog();
        break;
      case 'new-tab':
        app.newTab();
        break;
      case 'close-tab':
        // ⌘W closes the topmost viewer first (one level, as Esc does); the tab once the graph shows.
        if (repo && repo.view.kind !== 'graph') {
          hideContextMenu();
          repo.back();
        } else {
          app.closeTab();
        }
        break;
      case 'refresh':
        if (repo) void repo.refresh('all');
        break;
      case 'find':
        find();
        break;
      case 'toggle-details':
        settings.detailsVisible = !settings.detailsVisible;
        break;
      case 'settings':
        settingsOpen = !settingsOpen;
        break;
      case 'zoom-in':
        zoom(1);
        break;
      case 'zoom-out':
        zoom(-1);
        break;
      case 'zoom-reset':
        zoom(0);
        break;
      default: {
        const unknown: never = id;
        void unknown;
      }
    }
  }

  function onDragDrop(e: DragDropEvent) {
    switch (e.type) {
      case 'enter':
        dropping = e.paths.length > 0;
        break;
      case 'drop':
        dropping = false;
        if (e.paths.length > 0) void openPaths(e.paths);
        break;
      case 'leave':
        dropping = false;
        break;
    }
  }

  // ───────────── window focus: user.css + refresh ─────────────

  let userCssSeq = 0;

  /** (Re)load the user stylesheet into <style id="user-css">, kept last in <head>. */
  async function applyUserCss() {
    const seq = ++userCssSeq;
    let css: string;
    try {
      css = (await api.userCss()).css ?? '';
    } catch {
      return;
    }
    if (seq !== userCssSeq) return;
    let el = document.getElementById('user-css');
    if (!(el instanceof HTMLStyleElement)) {
      el = document.createElement('style');
      el.id = 'user-css';
    }
    if (el.textContent !== css) el.textContent = css;
    if (document.head.lastElementChild !== el) document.head.appendChild(el);
    refreshTheme();
  }

  let lastFocusRefresh = Date.now();

  /** FSEvents can miss changes across sleep: refresh the active repo on focus (throttled). */
  function onWindowFocus() {
    void applyUserCss();
    const repo = app.active?.repo;
    const now = Date.now();
    if (!repo?.ready || now - lastFocusRefresh < FOCUS_REFRESH_MS) return;
    lastFocusRefresh = now;
    void repo.refresh('all');
  }

  // ───────────── keyboard ─────────────

  interface EscSnapshot {
    event: KeyboardEvent;
    repo: RepoStore;
    view: CenterView;
    selection: Selection;
    search: unknown;
  }

  /** Active repo state when an Esc started propagating: tells whether an owner already handled it. */
  let escSnapshot: EscSnapshot | null = null;

  function onKeydownCapture(e: KeyboardEvent) {
    escSnapshot = null;
    if (e.code !== 'Escape') return;
    const repo = app.active?.repo;
    if (repo) escSnapshot = { event: e, repo, view: repo.view, selection: repo.selection, search: repo.graph.search };
  }

  /** ⌘1…⌘9 → 1…9, otherwise 0. */
  function tabNumber(e: KeyboardEvent): number {
    for (let n = 1; n <= 9; n++) if (matches(e, `mod+Digit${n}`, `mod+Numpad${n}`)) return n;
    return 0;
  }

  function onKeydown(e: KeyboardEvent) {
    if (e.defaultPrevented || isComposing(e)) return;
    const n = tabNumber(e);
    if (n > 0) {
      e.preventDefault();
      app.activateIndex(n - 1);
    } else if (matches(e, 'ctrl+Tab', 'mod+shift+BracketRight')) {
      e.preventDefault();
      app.cycle(1);
    } else if (matches(e, 'ctrl+shift+Tab', 'mod+shift+BracketLeft')) {
      e.preventDefault();
      app.cycle(-1);
    } else if (matches(e, 'Escape')) {
      const snap = escSnapshot?.event === e ? escSnapshot : null;
      escSnapshot = null;
      // This listener is registered before every component's window listener (graph search box,
      // viewers), so it runs first: decide only once the whole dispatch is over.
      if (snap) setTimeout(() => escapeFallback(e, snap));
    }
  }

  /**
   * Last layer of the Esc stack, for an Esc no owner handled (e.g. focus on the page body):
   * step back one viewer level (the graph last; a held Esc steps once), else drop the compare
   * partner. Context menu, popover, search box and viewers handle their own Esc first (and
   * preventDefault it).
   */
  function escapeFallback(e: KeyboardEvent, snap: EscSnapshot) {
    const repo = app.active?.repo;
    if (e.defaultPrevented || !repo || snap.repo !== repo || isEditable(e.target)) return;
    if (repo.view !== snap.view || repo.selection !== snap.selection || repo.graph.search !== snap.search) return;
    const sel = repo.selection;
    if (repo.view.kind !== 'graph') {
      if (!e.repeat) repo.back();
    } else if (sel.kind === 'compare') {
      const row = repo.graph.selectedRow;
      if (row != null) void repo.selectRow(row);
      else {
        repo.graph.compareRow = null;
        // '' is the working-tree end of a commit ↔ WIP compare.
        repo.select(sel.to === '' ? { kind: 'wip', cwd: repo.id } : { kind: 'commit', oid: sel.to });
      }
    }
  }

  /** Suppress WebKit's page menu (Reload, …) except over text fields and selected text. */
  function onContextMenu(e: MouseEvent) {
    if (e.defaultPrevented || isEditable(e.target)) return;
    const sel = window.getSelection();
    if (sel && !sel.isCollapsed && e.target instanceof Node && sel.containsNode(e.target, true)) return;
    e.preventDefault();
  }

  // ───────────── effects ─────────────

  // Graph-affecting settings reload every open graph (debounced; not on startup).
  let graphKey: string | null = null;
  $effect(() => {
    const key = JSON.stringify([
      settings.graphOrder,
      settings.maxCommits,
      settings.showRemotes,
      settings.showTags,
      settings.showStashes,
      settings.pinHeadLane,
    ]);
    if (graphKey === null) {
      graphKey = key;
      return;
    }
    if (key === graphKey) return;
    const timer = setTimeout(() => {
      graphKey = key;
      for (const tab of app.tabs) void tab.repo?.reloadGraph();
    }, GRAPH_RELOAD_MS);
    return () => clearTimeout(timer);
  });

  // Webview zoom. TabBar holds its traffic-light clearance in points with this same clamped factor.
  $effect(() => {
    const z = clampZoom(settings.zoom);
    try {
      getCurrentWebview()
        .setZoom(z)
        .catch(() => {});
    } catch {
      /* not running inside Tauri */
    }
  });

  // Pages in creation order (tab ids only grow), not tab order: reordering tabs must never move a
  // page's DOM — a re-inserted subtree loses its scroll positions and focus.
  const pages = $derived([...app.tabs].sort((a, b) => a.id - b.id));

  // Each tab remembers its last focused element; switching tabs restores it.
  const focusMemory = new Map<number, HTMLElement>();

  function rememberFocus(id: number, e: FocusEvent) {
    if (e.target instanceof HTMLElement) focusMemory.set(id, e.target);
  }

  $effect(() => {
    const id = app.activeId;
    untrack(() => {
      hideContextMenu();
      for (const k of focusMemory.keys()) if (!app.tabs.some((t) => t.id === k)) focusMemory.delete(k);
      const a = document.activeElement;
      if (a instanceof HTMLElement && a.closest('[inert]')) a.blur();
      if (document.activeElement && document.activeElement !== document.body) return;
      const el = id == null ? undefined : focusMemory.get(id);
      if (el && el.isConnected && !el.closest('[inert]')) el.focus({ preventScroll: true });
    });
  });
</script>

<svelte:window onkeydowncapture={onKeydownCapture} onkeydown={onKeydown} oncontextmenu={onContextMenu} />

<div class="app">
  <TabBar bind:settingsOpen onfind={find} />
  <div class="pages">
    {#each pages as tab (tab.id)}
      <div
        class="page"
        class:hidden={tab.id !== app.activeId}
        inert={tab.id !== app.activeId}
        onfocusin={(e) => rememberFocus(tab.id, e)}
      >
        {#if tab.repo}
          {#key tab.repo}
            <RepoView repo={tab.repo} />
          {/key}
        {:else}
          <Welcome active={tab.id === app.activeId} />
        {/if}
      </div>
    {/each}
  </div>
  <Footer />
  {#if dropping}<div class="drop" aria-hidden="true"></div>{/if}
</div>

<RefPopover />
<ContextMenu />
<Toast />

<style>
  .app {
    height: 100%;
    display: flex;
    flex-direction: column;
    background: var(--bg-1);
  }

  .pages {
    position: relative;
    flex: 1 1 auto;
    min-height: 0;
  }

  .page {
    position: absolute;
    inset: 0;
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }
  /* opacity also hides descendants that force `visibility: visible`. Out of the window as well, at the
     same size (the layout inside does not change): WebKit routes the wheel by the frontmost layer under
     the pointer and a hidden page's layers still count, so one stacked above the shown page would
     swallow the wheel over its lists. */
  .page.hidden {
    visibility: hidden;
    opacity: 0;
    left: -300vw;
    right: 300vw;
  }

  .drop {
    position: fixed;
    inset: 0;
    z-index: 900;
    pointer-events: none;
    outline: 2px solid var(--accent);
    outline-offset: -2px;
  }
</style>
