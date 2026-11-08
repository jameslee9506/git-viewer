<!--
  Title bar: repo tabs (pointer-drag to reorder, middle-click to close) and the global toggles.
  Empty bar space drags the window (data-tauri-drag-region); the traffic lights sit in the left
  padding (78pt, 8px in fullscreen). The webview zoom scales CSS px but not the native lights, so
  that padding and the bar's minimum height (its 100% height) are held in points at every zoom.
-->
<script lang="ts">
  import { onMount, tick } from 'svelte';
  import { flip } from 'svelte/animate';
  import { getCurrentWindow } from '@tauri-apps/api/window';
  import { revealItemInDir } from '@tauri-apps/plugin-opener';
  import Copy from '@lucide/svelte/icons/copy';
  import FolderGit2 from '@lucide/svelte/icons/folder-git-2';
  import FolderOpen from '@lucide/svelte/icons/folder-open';
  import PanelRight from '@lucide/svelte/icons/panel-right';
  import Plus from '@lucide/svelte/icons/plus';
  import Search from '@lucide/svelte/icons/search';
  import Settings from '@lucide/svelte/icons/settings';
  import X from '@lucide/svelte/icons/x';
  import { errorMessage } from '../../api';
  import { app, type Tab } from '../../state/app.svelte';
  import { settings } from '../../state/settings.svelte';
  import { showContextMenu, toast, type MenuEntry } from '../../state/ui.svelte';
  import type { RepoOperation } from '../../types';
  import { copyText } from '../../util/format';
  import SettingsPopover, { clampZoom } from './SettingsPopover.svelte';

  interface Props {
    /** Settings popover visibility (the ⌘, menu item toggles it too). */
    settingsOpen?: boolean;
    /** Open the active repo's graph search. */
    onfind: () => void;
  }

  let { settingsOpen = $bindable(false), onfind }: Props = $props();

  /** Loading must last this long before the progress line shows (quick refreshes stay silent). */
  const PROGRESS_DELAY_MS = 300;
  /** Pointer travel before a press on a tab turns into a reorder drag. */
  const DRAG_THRESHOLD = 4;

  interface Press {
    id: number;
    pointerId: number;
    startX: number;
    x: number;
    /** Pointer offset from the tab's left edge at press time. */
    grab: number;
    el: HTMLElement;
  }

  let fullscreen = $state(false);
  let progress = $state(false);
  let strip: HTMLDivElement | undefined = $state();
  let gear: HTMLButtonElement | undefined = $state();
  /** Tab being dragged and its offset from its slot. */
  let dragId = $state<number | null>(null);
  let dragDx = $state(0);

  let press: Press | null = null;
  let frame = 0;
  let middleId: number | null = null;

  const activeRepo = $derived(app.active?.repo ?? null);
  const busy = $derived(!!activeRepo && (activeRepo.graph.loading || !activeRepo.ready));
  /** Webview zoom as App applies it (setZoom); `.tabbar` divides its traffic-light clearance by it. */
  const zoom = $derived(clampZoom(settings.zoom));

  /**
   * Tab groups (features-ux M14): a run of adjacent tabs sharing a commonDir (one repo plus its
   * worktrees) gets a shared underline. Tab id → its place in the run; a lone tab is not listed.
   */
  const groupEdge = $derived.by(() => {
    const key = (i: number) => app.tabs[i]?.repo?.info.commonDir;
    const edges = new Map<number, 'start' | 'mid' | 'end'>();
    for (let i = 0; i < app.tabs.length; i++) {
      const k = key(i);
      if (!k) continue;
      const prev = key(i - 1) === k;
      const next = key(i + 1) === k;
      if (prev || next) edges.set(app.tabs[i].id, prev && next ? 'mid' : prev ? 'end' : 'start');
    }
    return edges;
  });

  $effect(() => {
    if (!busy) {
      progress = false;
      return;
    }
    const timer = setTimeout(() => (progress = true), PROGRESS_DELAY_MS);
    return () => clearTimeout(timer);
  });

  // Keep the active tab scrolled into view (not while a drag is moving tabs around).
  $effect(() => {
    const id = app.activeId;
    const s = strip;
    void app.tabs;
    if (id == null || !s || dragId !== null) return;
    const el = s.querySelector<HTMLElement>(`[data-tab="${id}"]`);
    if (!el) return;
    const left = el.offsetLeft;
    const right = left + el.offsetWidth;
    if (left < s.scrollLeft) s.scrollLeft = left;
    else if (right > s.scrollLeft + s.clientWidth) s.scrollLeft = right - s.clientWidth;
  });

  // Fullscreen hides the traffic lights: drop their padding.
  onMount(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    let timer: ReturnType<typeof setTimeout> | undefined;
    try {
      const win = getCurrentWindow();
      const check = () => {
        win.isFullscreen().then(
          (f) => {
            if (!disposed) fullscreen = f;
          },
          () => {},
        );
      };
      check();
      win
        .onResized(() => {
          clearTimeout(timer);
          timer = setTimeout(check, 80);
        })
        .then(
          (u) => (disposed ? u() : (unlisten = u)),
          () => {},
        );
    } catch {
      /* not running inside Tauri */
    }
    return () => {
      disposed = true;
      clearTimeout(timer);
      unlisten?.();
      endPress();
    };
  });

  function opTitle(op: RepoOperation): string {
    return op.step != null && op.total != null ? `${op.kind} ${op.step}/${op.total}` : op.kind;
  }

  /** Others slide into place when tabs move; the dragged tab follows the pointer instead. */
  function tabFlip(node: HTMLElement, rects: { from: DOMRect; to: DOMRect }) {
    if (dragId !== null && node.dataset.tab === String(dragId)) return { duration: 0 };
    return flip(node, rects, { duration: 150 });
  }

  function onTabPointerDown(e: PointerEvent, id: number) {
    if (e.button === 1) {
      e.preventDefault();
      middleId = id;
      return;
    }
    if (e.button !== 0 || !(e.currentTarget instanceof HTMLElement)) return;
    // A new press means the previous one ended where we did not see its release.
    if (press) endPress();
    app.activate(id);
    const el = e.currentTarget;
    press = {
      id,
      pointerId: e.pointerId,
      startX: e.clientX,
      x: e.clientX,
      grab: e.clientX - el.getBoundingClientRect().left,
      el,
    };
    window.addEventListener('pointermove', onPressMove, true);
    window.addEventListener('pointerup', onPressUp, true);
    window.addEventListener('pointercancel', onPressUp, true);
    window.addEventListener('blur', endPress);
  }

  function onTabPointerUp(e: PointerEvent, id: number) {
    if (e.button !== 1) return;
    if (middleId === id) {
      e.preventDefault();
      app.closeTab(id);
    }
    middleId = null;
  }

  function onPressMove(e: PointerEvent) {
    const p = press;
    if (!p || e.pointerId !== p.pointerId) return;
    // Released where no pointerup reached us (outside the window): stop following the pointer.
    if ((e.buttons & 1) === 0) {
      endPress();
      return;
    }
    p.x = e.clientX;
    if (dragId === null) {
      if (Math.abs(p.x - p.startX) < DRAG_THRESHOLD) return;
      dragId = p.id;
    }
    if (!frame) frame = requestAnimationFrame(() => void dragFrame());
  }

  function onPressUp(e: PointerEvent) {
    if (press && e.pointerId === press.pointerId) endPress();
  }

  function endPress() {
    press = null;
    if (frame) cancelAnimationFrame(frame);
    frame = 0;
    dragId = null;
    dragDx = 0;
    window.removeEventListener('pointermove', onPressMove, true);
    window.removeEventListener('pointerup', onPressUp, true);
    window.removeEventListener('pointercancel', onPressUp, true);
    window.removeEventListener('blur', endPress);
  }

  /** Put the dragged tab under the pointer and move it to the slot its center is over. */
  async function dragFrame() {
    frame = 0;
    const p = press;
    const s = strip;
    if (!p || !s || dragId !== p.id) return;
    const tabs = [...s.querySelectorAll<HTMLElement>('[data-tab]')];
    const first = tabs[0];
    const last = tabs[tabs.length - 1];
    if (!first || !last) return;
    const me = p.el;
    // offsetLeft ignores transforms: these are the tabs' slots, not where they are drawn.
    const base = s.getBoundingClientRect().left - s.scrollLeft;
    const width = me.offsetWidth;
    const minLeft = base + first.offsetLeft;
    const maxLeft = base + last.offsetLeft + last.offsetWidth - width;
    const left = Math.max(minLeft, Math.min(maxLeft, p.x - p.grab));
    const center = left + width / 2;
    let to = 0;
    for (const t of tabs) {
      if (t !== me && base + t.offsetLeft + t.offsetWidth / 2 < center) to++;
    }
    if (app.tabs.findIndex((t) => t.id === p.id) !== to) {
      app.moveTab(p.id, to);
      await tick();
      if (press !== p) return;
    }
    dragDx = left - (base + me.offsetLeft);
  }

  function onTabContextMenu(e: MouseEvent, tab: Tab) {
    const items: MenuEntry[] = [
      {
        label: 'Close',
        icon: X,
        hint: tab.id === app.activeId ? '⌘W' : undefined,
        action: () => app.closeTab(tab.id),
      },
      {
        label: 'Close others',
        disabled: app.tabs.length < 2,
        action: () => {
          app.activate(tab.id);
          for (const t of app.tabs) if (t.id !== tab.id) app.closeTab(t.id);
        },
      },
    ];
    const path = tab.repo?.id;
    if (path) {
      items.push(
        { separator: true },
        {
          label: 'Copy path',
          icon: Copy,
          action: () => {
            void copyText(path).then(() => toast('Copied'));
          },
        },
        {
          label: 'Reveal in Finder',
          icon: FolderOpen,
          action: () => {
            revealItemInDir(path).catch((err) => toast(errorMessage(err)));
          },
        },
      );
    }
    showContextMenu(e, items);
  }

  function onWheel(e: WheelEvent) {
    const s = strip;
    if (!s || s.scrollWidth <= s.clientWidth || Math.abs(e.deltaX) >= Math.abs(e.deltaY)) return;
    e.preventDefault();
    s.scrollLeft += e.deltaY;
  }
</script>

<!-- Clicks in the bar never take keyboard focus from the page (graph / lists keep their keys). -->
<!-- svelte-ignore a11y_no_static_element_interactions -->
<header
  class="tabbar"
  class:fullscreen
  style:--zoom={zoom}
  data-tauri-drag-region
  onmousedown={(e) => e.preventDefault()}
>
  <div class="strip" class:dragging={dragId !== null} bind:this={strip} role="tablist" onwheel={onWheel}>
    {#each app.tabs as tab (tab.id)}
      {@const active = tab.id === app.activeId}
      {@const edge = groupEdge.get(tab.id)}
      <div
        class="tab"
        class:active
        class:grouped={!!edge}
        class:group-end={edge === 'end'}
        class:dragging={dragId === tab.id}
        role="tab"
        tabindex="-1"
        aria-selected={active}
        data-tab={tab.id}
        title={tab.repo?.info.path}
        style:transform={dragId === tab.id ? `translateX(${dragDx}px)` : null}
        animate:tabFlip
        onpointerdown={(e) => onTabPointerDown(e, tab.id)}
        onpointerup={(e) => onTabPointerUp(e, tab.id)}
        oncontextmenu={(e) => onTabContextMenu(e, tab)}
      >
        {#if tab.repo}
          {@const op = tab.repo.status?.operation ?? null}
          {#if op}<span class="op" title={opTitle(op)}></span>{/if}
          {#if tab.repo.info.isLinkedWorktree}
            <span class="glyph"><FolderGit2 size={12} /></span>
          {/if}
          <span class="name ellipsis">{tab.repo.info.name}</span>
        {:else}
          <span class="name glyph"><FolderOpen size={12} /></span>
        {/if}
        <button
          class="close"
          title={active ? 'Close ⌘W' : 'Close'}
          onpointerdown={(e) => {
            if (e.button === 0) e.stopPropagation();
          }}
          onclick={() => app.closeTab(tab.id)}
        >
          <X size={12} />
        </button>
        {#if active && progress}<span class="progress" aria-hidden="true"></span>{/if}
      </div>
    {/each}
  </div>

  <button class="icon-btn" title="New tab ⌘T" onclick={() => app.newTab()}><Plus /></button>

  <div class="fill" data-tauri-drag-region></div>

  <div class="actions" data-tauri-drag-region>
    <button class="icon-btn" title="Find ⌘F" disabled={!activeRepo} onclick={() => onfind()}><Search /></button>
    <button
      class="icon-btn"
      class:on={settings.detailsVisible}
      title="Details ⌘K"
      aria-pressed={settings.detailsVisible}
      onclick={() => (settings.detailsVisible = !settings.detailsVisible)}
    >
      <PanelRight />
    </button>
    <div class="anchor">
      <button
        class="icon-btn"
        class:on={settingsOpen}
        bind:this={gear}
        title="Settings ⌘,"
        aria-expanded={settingsOpen}
        onclick={() => (settingsOpen = !settingsOpen)}
      >
        <Settings />
      </button>
      {#if settingsOpen}
        <SettingsPopover anchor={gear} onclose={() => (settingsOpen = false)} />
      {/if}
    </div>
  </div>
</header>

<style>
  .tabbar {
    position: relative;
    z-index: 20;
    flex: none;
    display: flex;
    align-items: center;
    gap: 4px;
    /* The traffic lights (trafficLightPosition 14,17) stay fixed in points while the webview zoom
       scales every CSS px. Dividing by --zoom keeps the tabs 78pt from the window edge (the lights
       end at ≈68pt) and, zoomed out, keeps the bar at its 100% height in points so the lights stay
       centred in it. */
    height: max(var(--tabbar-h), calc(var(--tabbar-h) / var(--zoom, 1)));
    padding: 0 8px 0 calc(78px / var(--zoom, 1));
    background: var(--bg-0);
    border-bottom: 1px solid var(--border);
  }
  /* Fullscreen hides the lights: plain zoomed metrics. */
  .tabbar.fullscreen {
    height: var(--tabbar-h);
    padding-left: 8px;
  }

  .strip {
    position: relative;
    flex: 0 1 auto;
    min-width: 0;
    height: 100%;
    display: flex;
    align-items: center;
    gap: 2px;
    overflow-x: auto;
    overflow-y: hidden;
  }
  .strip::-webkit-scrollbar {
    display: none;
  }

  .tab {
    position: relative;
    flex: 0 1 auto;
    display: flex;
    align-items: center;
    gap: 6px;
    min-width: 96px;
    max-width: 200px;
    height: 26px;
    padding: 0 4px 0 10px;
    border-radius: 5px;
    color: var(--text-2);
    font-size: var(--fs-sm);
    transition:
      transform 120ms ease-out,
      color var(--dur),
      background var(--dur);
  }
  .tab:hover {
    color: var(--text-1);
  }
  .tab.active {
    background: var(--bg-1);
    color: var(--text-0);
  }
  .tab.dragging {
    z-index: 1;
    transition: none;
  }
  .strip.dragging .tab {
    pointer-events: none;
  }

  /* Shared underline of a tab group; right: -2px bridges the strip gap to the next tab of the run.
     The progress line comes later in the tab and draws on top. */
  .tab.grouped::before {
    content: '';
    position: absolute;
    left: 0;
    right: -2px;
    bottom: 0;
    height: 2px;
    background: var(--border-strong);
    pointer-events: none;
  }
  .tab.group-end::before {
    right: 0;
  }

  .name {
    flex: 1 1 auto;
  }

  .glyph {
    display: inline-flex;
    flex: none;
    color: var(--text-3);
  }
  .tab.active .glyph {
    color: var(--text-2);
  }

  .op {
    flex: none;
    width: 5px;
    height: 5px;
    border-radius: 50%;
    background: var(--accent);
  }

  .close {
    flex: none;
    display: grid;
    place-items: center;
    width: 18px;
    height: 18px;
    border-radius: var(--radius-sm);
    color: var(--text-2);
    visibility: hidden;
  }
  .tab:hover .close {
    visibility: visible;
  }
  .close:hover {
    background: var(--bg-hover);
    color: var(--text-0);
  }

  .progress {
    position: absolute;
    left: 8px;
    right: 8px;
    bottom: 0;
    height: 2px;
    overflow: hidden;
    border-radius: 1px;
    pointer-events: none;
  }
  .progress::after {
    content: '';
    position: absolute;
    top: 0;
    bottom: 0;
    left: 0;
    width: 40%;
    border-radius: 1px;
    background: var(--accent);
    animation: slide 1s ease-in-out infinite;
  }
  @keyframes slide {
    from {
      transform: translateX(-100%);
    }
    to {
      transform: translateX(250%);
    }
  }

  .fill {
    flex: 1 1 auto;
    align-self: stretch;
    min-width: 24px;
  }

  .actions {
    flex: none;
    display: flex;
    align-items: center;
    gap: 2px;
    align-self: stretch;
  }

  .anchor {
    position: relative;
    display: flex;
  }
</style>
