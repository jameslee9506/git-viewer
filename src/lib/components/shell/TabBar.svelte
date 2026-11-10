<!--
  Title bar: the tabs — repositories, new tabs and groups of them — and the global toggles. Every
  tab starts with the icon of its kind, and the active one is filled like a browser's tab: clear of
  the bar's top, its top corners rounded, standing on the bar's bottom edge. A group is one cell
  until it is opened: then its tabs unfold to the right of its name, on a tinted tray that holds them
  and stands on that edge too. They unfold and fold back in motion, the cells after them riding along.
  The cells outside it keep their size; a row too long for the bar scrolls. A pointer drag reorders,
  takes a tab onto the open group's tray or off it, and drops one onto a closed group; middle-click
  closes a tab; double-click names a group. The `+` after the tabs opens a menu under it: a new tab,
  or a new group.
  Empty bar space drags the window (data-tauri-drag-region); the traffic lights sit in the left
  padding (78pt, 8px in fullscreen). The webview zoom scales CSS px but not the native lights, so
  that padding and the bar's minimum height (its 100% height) are held in points at every zoom.
-->
<script lang="ts">
  import { onMount, tick, untrack } from 'svelte';
  import { flip } from 'svelte/animate';
  import { getCurrentWindow } from '@tauri-apps/api/window';
  import { revealItemInDir } from '@tauri-apps/plugin-opener';
  import Copy from '@lucide/svelte/icons/copy';
  import Folder from '@lucide/svelte/icons/folder';
  import FolderInput from '@lucide/svelte/icons/folder-input';
  import FolderOpen from '@lucide/svelte/icons/folder-open';
  import FolderOutput from '@lucide/svelte/icons/folder-output';
  import FolderPlus from '@lucide/svelte/icons/folder-plus';
  import FolderX from '@lucide/svelte/icons/folder-x';
  import GitBranch from '@lucide/svelte/icons/git-branch';
  import GitFork from '@lucide/svelte/icons/git-fork';
  import PanelRight from '@lucide/svelte/icons/panel-right';
  import Pencil from '@lucide/svelte/icons/pencil';
  import Plus from '@lucide/svelte/icons/plus';
  import Search from '@lucide/svelte/icons/search';
  import Settings from '@lucide/svelte/icons/settings';
  import SquareDashed from '@lucide/svelte/icons/square-dashed';
  import Ungroup from '@lucide/svelte/icons/ungroup';
  import X from '@lucide/svelte/icons/x';
  import { errorMessage } from '../../api';
  import { app, isGroup, type Group, type Place } from '../../state/app.svelte';
  import { settings } from '../../state/settings.svelte';
  import { contextMenu, hideContextMenu, showContextMenu, showMenu, toast, type MenuEntry } from '../../state/ui.svelte';
  import type { RepoOperation } from '../../types';
  import { copyText } from '../../util/format';
  import { isComposing, isEditable } from '../../util/keys';
  import SettingsPopover, { clampZoom } from './SettingsPopover.svelte';
  import {
    layoutCells,
    placeGroup,
    placeTab,
    unitEnd,
    type Cell,
    type Folding,
    type GroupCell,
    type Slot,
    type TabCell,
  } from './tabcells';
  import UpdateButton from './UpdateButton.svelte';

  interface Props {
    /** Settings popover visibility (the ⌘, menu item toggles it too). */
    settingsOpen?: boolean;
    /** Open the active repo's graph search. */
    onfind: () => void;
    /** A group's name was typed: the keyboard focus goes back to the page. */
    onrefocus: () => void;
  }

  let { settingsOpen = $bindable(false), onfind, onrefocus }: Props = $props();

  /** Loading must last this long before the progress line shows (quick refreshes stay silent). */
  const PROGRESS_DELAY_MS = 300;
  /** Pointer travel before a press on a cell turns into a drag. */
  const DRAG_THRESHOLD = 4;
  /** A second click this soon after the first, in the same spot, makes a double-click. */
  const DOUBLE_CLICK_MS = 400;
  /** A tab without a repository. */
  const NEW_TAB = 'New Tab';
  /** Gap between the `+` button and the menu it opens under it. */
  const MENU_GAP = 4;
  /** How long a group takes to unfold, or to fold away. */
  const FOLD_MS = 240;
  /** ...and how: setting off gently, quick on the way, slow into place. */
  const FOLD_EASE = 'ease';

  interface Press {
    /** The pressed cell: a tab's or a group's id. */
    id: number;
    /** A group: it moves among the top-level items, with its tabs when it is open. */
    group: boolean;
    pointerId: number;
    startX: number;
    x: number;
    /** Where on the cell the pointer went down, as a share of its width. */
    grab: number;
    /** What the press does once released without a drag (see onCellPointerDown). */
    click: (() => void) | null;
  }

  let fullscreen = $state(false);
  let progress = $state(false);
  let strip: HTMLDivElement | undefined = $state();
  let gear: HTMLButtonElement | undefined = $state();
  /** The cell being dragged and its offset from its slot. */
  let dragId = $state<number | null>(null);
  let dragDx = $state(0);
  /**
   * The group shown open while a drag lasts. The bar keeps its shape until the drop, wherever the
   * active tab is dragged: only then does the open group follow it.
   */
  let heldGroup = $state<number | null>(null);
  /** The closed group the dragged tab is over: released there, it goes into it. */
  let dropInto = $state<number | null>(null);
  /** The groups that closed a moment ago: their tabs stay in the row until they are folded away. */
  let folding = $state.raw<Folding[]>([]);
  /** The open group opened a moment ago: its tabs are unfolding. */
  let unfolding = $state(false);
  /** The text of the group name being typed (app.renaming). */
  let nameText = $state('');

  let press: Press | null = null;
  let frame = 0;
  let middleId: number | null = null;
  /**
   * The last click on a cell: where and when, the group it was on (if one), and whether it opened or
   * closed a group. That lays the bar out anew, so the second click of a double-click meets whatever
   * came under the pointer: double-clicks are told by the bar (onBarPointerDown), not by `dblclick`.
   */
  let lastClick: { at: number; x: number; y: number; group: number | null; relaid: boolean } | null = null;
  /** The cell a drag just let go of: it settles into its slot by its own transition (see cellFlip). */
  let settling: number | null = null;

  const activeRepo = $derived(app.active?.repo ?? null);
  const busy = $derived(!!activeRepo && (activeRepo.graph.loading || !activeRepo.ready));
  /** Webview zoom as App applies it (setZoom); `.tabbar` divides its traffic-light clearance by it. */
  const zoom = $derived(clampZoom(settings.zoom));

  const shownGroup = $derived(dragId !== null ? heldGroup : app.openGroupId);
  const cells = $derived(layoutCells(app.items, shownGroup, folding, unfolding));

  $effect(() => {
    if (!busy) {
      progress = false;
      return;
    }
    const timer = setTimeout(() => (progress = true), PROGRESS_DELAY_MS);
    return () => clearTimeout(timer);
  });

  // ───────────── fold ─────────────
  //
  // A group opens and closes in motion. Its tabs unfold from its cell, or fold back into it; its
  // tray grows and shrinks with them, fading in or out; the number beside its name tucks away or
  // comes back; and the cells after them ride along, moved by layout alone.
  //
  // The row itself is laid out at once for the new open group, as ever, with one addition: the tabs
  // of a group that closed stay in it (`folding`) — with no room, and nothing of them to see —
  // until they are folded away. The motion is laid over that: whatever the change turned (those
  // tabs and the ones that came, the groups' cells, counts and trays) is led from how it was drawn
  // before, by a snapshot, to how the row has it now — and the strip's scroll offset with them.
  // Such an animation leaves nothing behind: when it ends, or is stopped, the element is simply as
  // the row has it.

  /** What a fold leads of an element: its room along the row, and how much of it shows. */
  interface Box {
    width: number;
    padL: number;
    padR: number;
    marL: number;
    marR: number;
    opacity: number;
  }

  /** A cell as it was drawn before the open group changed. */
  interface Snap {
    box: Box;
    /** A tab's row of icon and name: its width. */
    label: number;
    /** A group's tab count; null without one. */
    count: Box | null;
    /** How much of a group's tray showed; null without one. */
    tray: number | null;
  }

  /** The row as it was drawn before the open group changed. */
  interface Drawn {
    cells: Map<string, Snap>;
    /** How far the strip was scrolled. */
    scroll: number;
  }

  /** Less motion was asked of the system: a group opens and closes at once. */
  const still = matchMedia('(prefers-reduced-motion: reduce)');
  /** The animations of a fold that still run, by the element each leads. */
  const glides = new Map<Element, Animation>();
  let foldFrame = 0;
  /** The group the row showed open when it was last laid out. */
  let shownLast: number | null | undefined;
  /** The row as it was drawn before the open group changed, until the fold begins from there. */
  let drawnBefore: Drawn | null = null;
  /**
   * Where a fold leads the strip's scroll offset: from where it was to where the row would have it
   * without a fold. `clock` is one of the fold's animations: it tells how far the fold is.
   */
  let scrolling: { from: number; to: number; clock: Animation } | null = null;

  const px = (n: number) => `${n}px`;
  const num = (v: string) => parseFloat(v) || 0;
  /** The same to the eye. */
  const near = (a: number, b: number) => Math.abs(a - b) < 0.05;

  function boxOf(el: Element): Box {
    const style = getComputedStyle(el);
    return {
      width: el.getBoundingClientRect().width,
      padL: num(style.paddingLeft),
      padR: num(style.paddingRight),
      marL: num(style.marginLeft),
      marR: num(style.marginRight),
      opacity: num(style.opacity),
    };
  }

  function alike(a: Box, b: Box): boolean {
    return (Object.keys(a) as (keyof Box)[]).every((k) => near(a[k], b[k]));
  }

  /** The box `b` as a keyframe: exactly that wide, whatever the content asks for. */
  const boxFrame = (b: Box): Keyframe => ({
    minWidth: px(b.width),
    maxWidth: px(b.width),
    paddingLeft: px(b.padL),
    paddingRight: px(b.padR),
    marginLeft: px(b.marL),
    marginRight: px(b.marR),
    opacity: b.opacity,
  });
  /** ...and its margins alone: of a cell that stays as wide as what it holds. */
  const edgeFrame = (b: Box): Keyframe => ({ marginLeft: px(b.marL), marginRight: px(b.marR) });

  /** Every cell of the strip `s` as it is drawn now, by the id of the cell. */
  function snapshot(s: HTMLElement): Map<string, Snap> {
    const snaps = new Map<string, Snap>();
    for (const [id, el] of cellEls(s)) {
      const count = el.querySelector('.count');
      const tray = el.querySelector('.tray');
      snaps.set(id, {
        box: boxOf(el),
        label: el.querySelector('.label')?.getBoundingClientRect().width ?? 0,
        count: count && boxOf(count),
        tray: tray && num(getComputedStyle(tray).opacity),
      });
    }
    return snaps;
  }

  // The open group changed. Before the row is laid out for it: note how the row is drawn, and keep
  // the tabs of the group that closed in it.
  $effect.pre(() => {
    const now = shownGroup;
    const was = shownLast;
    shownLast = now;
    if (was === undefined || was === now) return;
    untrack(() => {
      const s = strip;
      if (!s || still.matches) return;
      drawnBefore = { cells: snapshot(s), scroll: s.scrollLeft };
      const group = app.items.find((it): it is Group => isGroup(it) && it.id === was);
      /** The tabs its tray showed, in order. */
      const shown = [...s.querySelectorAll<HTMLElement>('.cell.in-tray:not(.leaving)')].map((el) =>
        Number(el.dataset.cell),
      );
      // A group folds with the tabs its tray showed, each where it was. One of them may have just
      // left it to stand right after it, at the top level (taken out of the group from its menu): it
      // keeps its place among the others meanwhile (layoutCells). A tab that left for elsewhere would
      // have the others between it and where it was: that group closes at once, as ever.
      const after = group && app.items[app.items.indexOf(group) + 1];
      const fold =
        group && shown.every((id) => group.tabs.some((t) => t.id === id) || after?.id === id)
          ? { id: group.id, tabs: shown }
          : null;
      folding = [...folding.filter((f) => f.id !== was && f.id !== now), ...(fold ? [fold] : [])];
      unfolding = now !== null;
    });
  });

  // ...and once it is laid out: lead it there. Ahead of the effects below, which size the trays and
  // scroll the strip: they are to find the cells where the fold has them begin.
  $effect(() => {
    void shownGroup;
    const was = drawnBefore;
    drawnBefore = null;
    const s = strip;
    if (was && s) untrack(() => startFold(s, was));
  });

  /** Lead `el` from one keyframe to the other. */
  function lead(el: Element, from: Keyframe, to: Keyframe): void {
    glides.set(el, el.animate([from, to], { duration: FOLD_MS, easing: FOLD_EASE }));
  }

  /**
   * The row in the strip `s` was laid out for another open group: lead what that turned from how it
   * was drawn (`was`) to how it is now.
   */
  function startFold(s: HTMLElement, was: Drawn): void {
    // What still moved stops here. Every element is then as the row has it — which is where it is
    // led to, from wherever it had got to (`was` has that).
    for (const glide of glides.values()) glide.cancel();
    glides.clear();
    scrolling = null;
    const gap = num(getComputedStyle(s).columnGap);
    /** A tab folded away: no room in the row — the gap before it taken back —, nothing to see. */
    const folded: Box = { width: 0, padL: 0, padR: 0, marL: -gap, marR: 0, opacity: 0 };
    const els = cellEls(s);
    // Everything is measured before anything is led: an animation changes the row from its start.
    const moves: (() => void)[] = [];
    for (const cell of cells) {
      const el = els.get(String(cell.id));
      if (!el) continue;
      const old = was.cells.get(String(cell.id));
      if (cell.kind === 'tab' && cell.fold !== null) {
        // A tab of a group that opened or closed. One that was not in the row unfolds from nothing;
        // the others go on from where they are — one that was whole stays so.
        const label = el.querySelector('.label');
        const from = old?.box ?? (cell.fold === 'in' ? folded : null);
        if (!from || !label) continue;
        const to = boxOf(el);
        if (alike(from, to)) continue;
        // Its icon and name keep their width: the cell's edge passes over them, nothing is squeezed.
        const wide = px(cell.fold === 'out' && old ? old.label : label.getBoundingClientRect().width);
        moves.push(() => {
          lead(el, boxFrame(from), boxFrame(to));
          lead(label, { minWidth: wide }, { minWidth: wide });
        });
        continue;
      }
      // Any other cell: the margin that an end of a tray gives it, or gave it. And of a group's
      // cell: its count, its tray.
      if (old) {
        const to = boxOf(el);
        if (!near(old.box.marL, to.marL) || !near(old.box.marR, to.marR)) {
          moves.push(() => lead(el, edgeFrame(old.box), edgeFrame(to)));
        }
      }
      const count = el.querySelector('.count');
      if (count && old?.count) {
        const from = old.count;
        const to = boxOf(count);
        if (!alike(from, to)) moves.push(() => lead(count, boxFrame(from), boxFrame(to)));
      }
      const tray = el.querySelector('.tray');
      if (tray) {
        const from = old?.tray ?? 0;
        const to = num(getComputedStyle(tray).opacity);
        if (!near(from, to)) moves.push(() => lead(tray, { opacity: from }, { opacity: to }));
      }
    }
    if (moves.length > 0) {
      // The strip, too, is led: from where it was scrolled to where it would be now without a fold
      // — as far as the row at rest lets it be (a shorter row takes the offset back at once, when it
      // is laid out), and with what shows in view.
      fitTrays(s);
      reveal(s);
      const to = s.scrollLeft;
      for (const move of moves) move();
      fitTrays(s);
      s.scrollLeft = was.scroll;
      const clock = glides.values().next().value;
      if (clock) scrolling = { from: was.scroll, to, clock };
    }
    if (!foldFrame) foldFrame = requestAnimationFrame(foldTick);
  }

  /**
   * A frame of a fold: the trays follow their cells, and the strip scrolls as far as the fold is.
   * Once nothing moves any more, the tabs that folded away leave the row.
   */
  function foldTick(): void {
    foldFrame = 0;
    for (const [el, glide] of glides) if (glide.playState !== 'running') glides.delete(el);
    const s = strip;
    if (s) {
      fitTrays(s);
      if (scrolling) {
        // How far the fold is, eased as its animations are; no answer once they are over. In whole
        // px: WebKit would cut the fraction off. No further than the row lets it at the moment.
        const done = scrolling.clock.effect?.getComputedTiming().progress ?? 1;
        s.scrollLeft = Math.round(scrolling.from + (scrolling.to - scrolling.from) * done);
      }
    }
    if (glides.size > 0) {
      foldFrame = requestAnimationFrame(foldTick);
      return;
    }
    scrolling = null;
    if (folding.length > 0) folding = [];
    unfolding = false;
  }

  /** The cells' elements in the strip `s`, by the id of their cell. */
  function cellEls(s: HTMLElement): Map<string, HTMLElement> {
    const els = new Map<string, HTMLElement>();
    for (const el of s.querySelectorAll<HTMLElement>('[data-cell]')) els.set(el.dataset.cell ?? '', el);
    return els;
  }

  /**
   * The elements of the cells of every tray in the strip `s` — the open group's, and that of each
   * group that is folding —: the group's own first, then its tabs'.
   */
  function trayRuns(s: HTMLElement): { open: boolean; els: HTMLElement[] }[] {
    const els = cellEls(s);
    const runs: { open: boolean; els: HTMLElement[] }[] = [];
    let run: HTMLElement[] | null = null;
    for (const cell of cells) {
      if (cell.tray === null) continue;
      const el = els.get(String(cell.id));
      if (cell.kind === 'group') {
        run = el ? [el] : null;
        if (run) runs.push({ open: cell.open, els: run });
      } else if (el) {
        run?.push(el);
      }
    }
    return runs;
  }

  /** The elements of the open group's cells in the strip `s`: the group's own first, then its tabs'. */
  function trayCells(s: HTMLElement): HTMLElement[] {
    return trayRuns(s).find((run) => run.open)?.els ?? [];
  }

  /**
   * Size every tray in the strip `s`. A tray reaches from its group's cell to the last of the
   * group's tabs, and as far past them as their outer margins go (`--tray-pad` at an end of the open
   * group's tray; on their way to it, or to nothing, while a group folds). So it is as wide as the
   * cells from the one to the other, the gaps between them and those two margins (the cells' widths,
   * which neither a drag nor a sliding cell changes) — less what a tab that folds takes back of the
   * gap before it, with its margin. The cells are the group's own, and with a folding group's a tab
   * that left it and keeps its place among them. Written to the tray itself: `--tray-l`, how far it
   * begins before the group's cell, and `--tray-w`.
   */
  function fitTrays(s: HTMLElement): void {
    const gap = parseFloat(getComputedStyle(s).columnGap) || 0;
    for (const { els } of trayRuns(s)) {
      const tray = els[0].querySelector<HTMLElement>('.tray');
      if (!tray) continue;
      const last = els[els.length - 1];
      const before = parseFloat(getComputedStyle(els[0]).marginLeft) || 0;
      let width = before;
      for (let el: Element | null = els[0]; el; el = el.nextElementSibling) {
        const style = getComputedStyle(el);
        width += el.getBoundingClientRect().width;
        if (el !== els[0]) width += gap + (parseFloat(style.marginLeft) || 0);
        if (el === last) {
          width += parseFloat(style.marginRight) || 0;
          break;
        }
      }
      tray.style.setProperty('--tray-l', `${before}px`);
      tray.style.setProperty('--tray-w', `${width}px`);
    }
  }

  // The trays' widths change with the cells of the row — this effect — or with the width of one of
  // their cells (a narrower window, a name being typed, a repository opened in a new tab): the
  // observer. While a group folds or unfolds, every frame sizes them too (foldTick). Ahead of the
  // effect below: a tray is part of what the strip scrolls.
  $effect(() => {
    const s = strip;
    if (!s) return;
    const runs = trayRuns(s);
    if (runs.length === 0) return;
    fitTrays(s);
    const observer = new ResizeObserver(() => fitTrays(s));
    for (const run of runs) for (const el of run.els) observer.observe(el);
    return () => observer.disconnect();
  });

  /** A cell's place in the strip's row, with its margins (the open group's tray ends in them). */
  function extent(el: HTMLElement): { left: number; right: number } {
    const style = getComputedStyle(el);
    return {
      left: el.offsetLeft - (parseFloat(style.marginLeft) || 0),
      right: el.offsetLeft + el.offsetWidth + (parseFloat(style.marginRight) || 0),
    };
  }

  /**
   * Scroll what shows into view in the strip `s`: the open group as a whole — its name with its
   * tabs — when the strip is wide enough for it, and in any case the active tab, or the cell of the
   * open group that has none. Not while a drag is moving cells around, nor while a group folds or
   * unfolds: a fold leads the strip to where this scrolls the row at rest (startFold).
   */
  function reveal(s: HTMLElement): void {
    const id = app.activeId ?? app.openGroupId;
    const group = trayCells(s);
    if (id == null || dragId !== null || glides.size > 0) return;
    const el = s.querySelector<HTMLElement>(`[data-cell="${id}"]`);
    if (!el) return;
    const show = (left: number, right: number) => {
      if (left < s.scrollLeft) s.scrollLeft = left;
      else if (right > s.scrollLeft + s.clientWidth) s.scrollLeft = right - s.clientWidth;
    };
    if (group.length > 0) {
      const left = extent(group[0]).left;
      const right = extent(group[group.length - 1]).right;
      if (right - left <= s.clientWidth) show(left, right);
    }
    const own = extent(el);
    show(own.left, own.right);
  }

  // Keep what shows in view whenever the row or the active tab changes.
  $effect(() => {
    if (strip) reveal(strip);
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
      cancelAnimationFrame(foldFrame);
      for (const glide of glides.values()) glide.cancel();
    };
  });

  /** The cell of what shows: the active tab's — or the open group's own, while that group has no tab. */
  function isActive(cell: Cell | undefined): boolean {
    if (!cell) return false;
    return cell.kind === 'tab' ? cell.id === app.activeId : cell.open && app.activeId == null;
  }

  function opTitle(op: RepoOperation): string {
    return op.step != null && op.total != null ? `${op.kind} ${op.step}/${op.total}` : op.kind;
  }

  /** Tooltip: a tab's path and its operation in progress; what a closed group holds. */
  function cellTitle(cell: Cell): string | undefined {
    const lines: string[] = [];
    if (cell.kind === 'group') {
      if (cell.open) return undefined;
      for (const t of cell.group.tabs) lines.push(t.repo?.info.name ?? NEW_TAB);
    } else {
      const repo = cell.tab.repo;
      if (repo) lines.push(repo.info.path);
      const op = repo?.status?.operation;
      if (op) lines.push(opTitle(op));
    }
    return lines.length > 0 ? lines.join('\n') : undefined;
  }

  // ───────────── drag ─────────────

  /**
   * A group opened or closed, or its tabs are folded away: the cells after them move with the fold,
   * by layout. None slides besides.
   */
  let relaid = false;
  $effect.pre(() => {
    void shownGroup;
    void folding;
    relaid = true;
    // Cleared after the animations of this update were set up (they are in a microtask).
    const timer = setTimeout(() => (relaid = false));
    return () => clearTimeout(timer);
  });

  /**
   * The others slide into place when cells move; the dragged one follows the pointer instead, and
   * the one just let go of settles by its CSS transition (flip would start from that transition's
   * first frame and hold the cell off its slot). A cell whose width changed takes its place at once
   * (flip would stretch its text on the way).
   */
  function cellFlip(node: HTMLElement, rects: { from: DOMRect; to: DOMRect }) {
    const still =
      relaid ||
      node.classList.contains('dragging') ||
      node.dataset.cell === String(settling) ||
      rects.from.width !== rects.to.width;
    return still ? { duration: 0 } : flip(node, rects, { duration: 150 });
  }

  /**
   * A press anywhere in the bar. The second click of a double-click is the bar's, whatever it lands
   * on: on a group it names the group, and after a click that opened or closed a group it is
   * dropped — that click moved another cell, a button or the bar's bare ground (where a double-click
   * zooms the window) under the pointer.
   */
  function onBarPointerDown(e: PointerEvent) {
    // Not a context click (a right press, or Ctrl-click).
    if (e.button !== 0 || e.ctrlKey) return;
    const last = lastClick;
    lastClick = null;
    if (
      !last ||
      (last.group == null && !last.relaid) ||
      e.timeStamp - last.at >= DOUBLE_CLICK_MS ||
      Math.abs(e.clientX - last.x) > DRAG_THRESHOLD ||
      Math.abs(e.clientY - last.y) > DRAG_THRESHOLD
    ) {
      return;
    }
    swallowPress(e);
    const group = last.group;
    // After this press: a mousedown in the bar ends a name being typed (onBarMouseDown).
    if (group != null) {
      setTimeout(() => {
        if (app.items.some((it) => isGroup(it) && it.id === group)) app.renaming = group;
      });
    }
  }

  /**
   * Keep the press that begins with `e` from everything else: the cells and buttons under it, and
   * the window's drag region (canceling the pointerdown holds back its mouse events; the click that
   * follows the release is stopped on its way down).
   */
  function swallowPress(e: PointerEvent) {
    e.preventDefault();
    e.stopPropagation();
    const types = ['mousedown', 'mouseup', 'click', 'dblclick'];
    const stop = (ev: Event) => {
      ev.preventDefault();
      ev.stopPropagation();
    };
    let timer: ReturnType<typeof setTimeout> | undefined;
    const done = () => {
      clearTimeout(timer);
      window.removeEventListener('pointerup', released, true);
      window.removeEventListener('pointercancel', released, true);
      for (const t of types) window.removeEventListener(t, stop, true);
    };
    // The click comes right after the release, before any timer.
    const released = () => {
      clearTimeout(timer);
      timer = setTimeout(done);
    };
    for (const t of types) window.addEventListener(t, stop, true);
    window.addEventListener('pointerup', released, true);
    window.addEventListener('pointercancel', released, true);
    // A release that never reaches the window must not leave the next click stopped.
    timer = setTimeout(done, 2000);
  }

  function onCellPointerDown(e: PointerEvent, cell: Cell) {
    // The group name being typed keeps its own presses.
    if (isEditable(e.target)) return;
    if (e.button === 1) {
      if (cell.kind !== 'tab') return;
      e.preventDefault();
      middleId = cell.id;
      return;
    }
    // Ctrl-click is the macOS context click: the menu's, like a right press.
    if (e.button !== 0 || e.ctrlKey || !(e.currentTarget instanceof HTMLElement)) return;
    // A new press means the previous one ended where we did not see its release.
    if (press) endPress();
    // A tab of the row that shows is active at once. A press that would open or close a group — a tab
    // outside the open group, a closed group — acts on release: until then it may turn into a drag,
    // which must not lay the bar out anew under the pointer.
    let click: (() => void) | null = null;
    if (cell.kind === 'group') {
      if (!cell.open) click = () => app.enter(cell.id);
    } else if ((cell.group?.id ?? null) === app.openGroupId) {
      app.activate(cell.id);
    } else {
      click = () => app.activate(cell.id);
    }
    const box = e.currentTarget.getBoundingClientRect();
    press = {
      id: cell.id,
      group: cell.kind === 'group',
      pointerId: e.pointerId,
      startX: e.clientX,
      x: e.clientX,
      grab: box.width > 0 ? (e.clientX - box.left) / box.width : 0,
      click,
    };
    window.addEventListener('pointermove', onPressMove, true);
    window.addEventListener('pointerup', onPressUp, true);
    window.addEventListener('pointercancel', onPressUp, true);
    window.addEventListener('blur', onWindowBlur);
  }

  function onCellPointerUp(e: PointerEvent, cell: Cell) {
    if (e.button !== 1) return;
    if (middleId === cell.id) {
      e.preventDefault();
      app.closeTab(cell.id);
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
      heldGroup = app.openGroupId;
      dragId = p.id;
    }
    if (!frame) frame = requestAnimationFrame(() => void dragFrame());
  }

  function onPressUp(e: PointerEvent) {
    const p = press;
    if (!p || e.pointerId !== p.pointerId) return;
    const released = e.type === 'pointerup';
    const clicked = released && dragId !== p.id;
    endPress(released ? dropInto : null);
    if (!clicked) return;
    p.click?.();
    lastClick = { at: e.timeStamp, x: e.clientX, y: e.clientY, group: p.group ? p.id : null, relaid: p.click !== null };
  }

  function onWindowBlur() {
    endPress();
  }

  /**
   * The press is over. A dragged tab stays where the drag put it — in the closed group `into` when
   * it was released over one — and a tab of the same repository beside it gives way.
   */
  function endPress(into: number | null = null) {
    const p = press;
    const dragged = p !== null && dragId === p.id;
    press = null;
    if (frame) cancelAnimationFrame(frame);
    frame = 0;
    dragId = null;
    dragDx = 0;
    heldGroup = null;
    dropInto = null;
    window.removeEventListener('pointermove', onPressMove, true);
    window.removeEventListener('pointerup', onPressUp, true);
    window.removeEventListener('pointercancel', onPressUp, true);
    window.removeEventListener('blur', onWindowBlur);
    if (!p || !dragged) return;
    const id = p.id;
    settling = id;
    // Until the animations of this update were set up (they are in a microtask).
    setTimeout(() => {
      if (settling === id) settling = null;
    });
    if (p.group) return;
    if (into != null) app.moveTo(id, into);
    else app.drop(id);
  }

  /** The cells with their slots along the bar. offsetLeft ignores transforms: where they belong, not where they are drawn. */
  function measure(s: HTMLElement): Slot[] {
    const base = s.getBoundingClientRect().left - s.scrollLeft;
    const els = cellEls(s);
    const slots: Slot[] = [];
    for (const cell of cells) {
      // A tab that is folding away is out of the row.
      if (cell.kind === 'tab' && cell.fold === 'out') continue;
      const el = els.get(String(cell.id));
      if (el) slots.push({ cell, left: base + el.offsetLeft, right: base + el.offsetLeft + el.offsetWidth });
    }
    return slots;
  }

  /** Where the dragged cell at `me` is drawn: under the pointer, inside the row; an open group with its tabs. */
  function drawn(slots: Slot[], me: number, p: Press): { left: number; right: number } {
    const slot = slots[me];
    const width = unitEnd(slots, me) - slot.left;
    const min = slots[0].left;
    const max = slots[slots.length - 1].right - width;
    const left = Math.max(min, Math.min(max, p.x - p.grab * (slot.right - slot.left)));
    return { left, right: left + width };
  }

  /** Put the dragged cell under the pointer and move it to where the pointer says it belongs. */
  async function dragFrame() {
    frame = 0;
    const p = press;
    const s = strip;
    if (!p || !s || dragId !== p.id) return;
    let slots: Slot[] = [];
    let me = -1;
    let into: number | null = null;
    // A move lays the row out anew, which may ask for one more (the tray's ends take a little room of
    // their own): a few rounds settle it while the pointer rests.
    for (let round = 0; round < 3; round++) {
      slots = measure(s);
      me = slots.findIndex((slot) => slot.cell.id === p.id);
      const at = app.placeOf(p.id);
      if (me < 0 || !at) return;
      let to: Place;
      into = null;
      if (p.group) {
        const { left, right } = drawn(slots, me, p);
        to = { group: null, index: placeGroup(slots, p.id, left, right) };
      } else {
        // The pointer as it is, also beyond the row's ends: past an open group that ends the row is
        // out of that group.
        ({ to, into } = placeTab(slots, p.id, p.x));
      }
      if (to.group === at.group && to.index === at.index) break;
      app.move(p.id, to);
      await tick();
      if (press !== p) return;
      me = -1;
    }
    if (me < 0) {
      slots = measure(s);
      me = slots.findIndex((slot) => slot.cell.id === p.id);
      if (me < 0) return;
    }
    dropInto = into;
    dragDx = drawn(slots, me, p).left - slots[me].left;
  }

  // ───────────── group names ─────────────

  /** The group `nameText` was set up for. */
  let nameFor: number | null = null;

  // The name input opens with the group's name. Going straight from one group's name to another's
  // (a new group made by its shortcut while typing) takes no blur: the first group gets its name here.
  $effect.pre(() => {
    const id = app.renaming;
    const prev = nameFor;
    nameFor = id;
    if (id == null) return;
    untrack(() => {
      if (prev != null && prev !== id) app.renameGroup(prev, nameText);
      nameText = app.items.find((it): it is Group => isGroup(it) && it.id === id)?.name ?? '';
    });
  });

  /** Focus a freshly rendered name input, its text selected: typing replaces it. */
  function selectAll(el: HTMLInputElement): void {
    el.focus();
    el.select();
  }

  /**
   * The typing ends: the group takes the name, or (`keep`) stays as it was — as it does when the
   * input was not up yet, and `nameText` is not its text. The page gets the keyboard focus back.
   * When the key `key` ended the typing, only once that key is up, and its repeats go nowhere until
   * then: they must not act in the page (Enter in the open screen's filter opens a repository).
   */
  function endRename(keep = false, key: string | null = null) {
    const id = app.renaming;
    if (id == null) return;
    if (keep || nameFor !== id) app.renaming = null;
    else app.renameGroup(id, nameText);
    if (key == null) {
      onrefocus();
      return;
    }
    const held = (e: KeyboardEvent) => {
      if (e.code !== key) return;
      e.preventDefault();
      e.stopPropagation();
    };
    const up = (e: Event) => {
      if (e instanceof KeyboardEvent && e.code !== key) return;
      window.removeEventListener('keydown', held, true);
      window.removeEventListener('keyup', up, true);
      window.removeEventListener('blur', up);
      onrefocus();
    };
    window.addEventListener('keydown', held, true);
    window.addEventListener('keyup', up, true);
    window.addEventListener('blur', up);
  }

  /** Keys typed in the name never reach the app's shortcuts. Enter names the group, Esc leaves its name. */
  function onNameKey(e: KeyboardEvent) {
    e.stopPropagation();
    if (isComposing(e)) return;
    if (e.code === 'Enter' || e.code === 'NumpadEnter') {
      e.preventDefault();
      endRename(false, e.code);
    } else if (e.code === 'Escape') {
      e.preventDefault();
      endRename(true, e.code);
    }
  }

  /**
   * Clicks in the bar never take the keyboard focus from the page (graph / lists keep their keys) —
   * but in the group name being typed, which such a click therefore cannot blur: it ends the typing.
   */
  function onBarMouseDown(e: MouseEvent) {
    if (isEditable(e.target)) return;
    e.preventDefault();
    endRename();
  }

  // ───────────── menus ─────────────

  function onCellContextMenu(e: MouseEvent, cell: Cell) {
    if (isEditable(e.target)) return;
    if (cell.kind === 'group') groupMenu(e, cell);
    else tabMenu(e, cell);
  }

  function tabMenu(e: MouseEvent, cell: TabCell) {
    const tab = cell.tab;
    const home = cell.group;
    const row = home ? home.tabs : app.items.filter((it) => !isGroup(it));
    const others = app.items.filter((it): it is Group => isGroup(it) && it !== home);
    const items: MenuEntry[] = [
      {
        label: 'Close',
        icon: X,
        hint: tab.id === app.activeId ? '⌘W' : undefined,
        action: () => app.closeTab(tab.id),
      },
      { label: 'Close others', disabled: row.length < 2, action: () => app.closeOthers(tab.id) },
      { separator: true },
      {
        label: 'Move to group',
        icon: FolderInput,
        children: [
          ...others.map((g): MenuEntry => ({ label: g.name, icon: Folder, action: () => app.moveTo(tab.id, g.id) })),
          { separator: true },
          { label: 'New group', icon: FolderPlus, action: () => app.newGroup(tab.id) },
        ],
      },
    ];
    if (home) {
      items.push({ label: 'Remove from group', icon: FolderOutput, action: () => app.moveTo(tab.id, null) });
    }
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

  function groupMenu(e: MouseEvent, cell: GroupCell) {
    const group = cell.group;
    const n = group.tabs.length;
    showContextMenu(e, [
      { label: 'Rename', icon: Pencil, action: () => (app.renaming = group.id) },
      { label: 'Ungroup', icon: Ungroup, disabled: n === 0, action: () => app.ungroup(group.id) },
      { separator: true },
      // Deleting its tabs with it asks first.
      { label: n > 0 ? 'Delete group…' : 'Delete group', icon: FolderX, action: () => void app.deleteGroup(group.id) },
    ]);
  }

  /** The menu the `+` button opened: its `contextMenu.seq`. */
  let addSeq = $state<number | null>(null);
  /** That menu is up. */
  const adding = $derived(addSeq !== null && contextMenu.open && contextMenu.seq === addSeq);

  /** The `+` button: what it adds — a tab, or a group — in a menu under it, or that menu away again. */
  function toggleAdd(e: MouseEvent) {
    if (adding) {
      hideContextMenu();
      return;
    }
    if (!(e.currentTarget instanceof Element)) return;
    const r = e.currentTarget.getBoundingClientRect();
    addSeq = showMenu(
      { x: r.left, y: r.bottom + MENU_GAP },
      [
        { label: 'New tab', icon: SquareDashed, hint: '⌘T', action: () => app.newTab() },
        { label: 'New group', icon: FolderPlus, hint: '⇧⌘N', action: () => app.newGroup() },
      ],
      e.currentTarget,
    );
  }

  function onWheel(e: WheelEvent) {
    const s = strip;
    if (!s || s.scrollWidth <= s.clientWidth || Math.abs(e.deltaX) >= Math.abs(e.deltaY)) return;
    e.preventDefault();
    s.scrollLeft += e.deltaY;
  }
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<header
  class="tabbar"
  class:fullscreen
  style:--zoom={zoom}
  data-tauri-drag-region
  onpointerdowncapture={onBarPointerDown}
  onmousedown={onBarMouseDown}
>
  <div class="strip" class:dragging={dragId !== null} bind:this={strip} role="tablist" onwheel={onWheel}>
    {#each cells as cell, i (cell.id)}
      <!-- An open group moves with its tabs. -->
      {@const moving = dragId !== null && (cell.id === dragId || (cell.kind === 'tab' && cell.group?.id === dragId))}
      <!-- A group is the active cell while it is open without a tab. -->
      {@const active = isActive(cell)}
      <!-- Its group closed: it folds away, out of reach. The tray it was on has no ends any more. -->
      {@const leaving = cell.kind === 'tab' && cell.fold === 'out'}
      {@const closing = leaving || (cell.kind === 'group' && cell.folding)}
      <div
        class="cell"
        class:chip={cell.kind === 'group'}
        class:open={cell.kind === 'group' && cell.open}
        class:editing={app.renaming === cell.id}
        class:in-tray={cell.kind === 'tab' && cell.tray !== null}
        class:fold={cell.kind === 'tab' && cell.fold !== null}
        class:leaving
        class:active
        class:sep={cell.sep && !active && !isActive(cells[i + 1])}
        class:run={cell.kind === 'tab' && cell.run !== null}
        class:run-end={cell.kind === 'tab' && cell.run === 'end'}
        class:tray-start={!closing && (cell.tray === 'start' || cell.tray === 'only')}
        class:tray-end={!closing && (cell.tray === 'end' || cell.tray === 'only')}
        class:dragging={moving}
        class:into={moving && dropInto !== null}
        class:drop={dropInto === cell.id}
        role="tab"
        tabindex="-1"
        inert={leaving}
        aria-selected={active}
        aria-expanded={cell.kind === 'group' ? cell.open : undefined}
        data-cell={cell.id}
        title={cellTitle(cell)}
        style:transform={moving ? `translateX(${dragDx}px)` : null}
        animate:cellFlip
        onpointerdown={(e) => onCellPointerDown(e, cell)}
        onpointerup={(e) => onCellPointerUp(e, cell)}
        oncontextmenu={(e) => onCellContextMenu(e, cell)}
      >
        {#if cell.kind === 'group'}
          {#if cell.open || cell.folding}
            <!-- The tray under the group's cells. This cell's own: it moves with the group, and stays
                 where it is while one of the group's tabs is dragged. Its width is set from the script.
                 It outlasts the group's closing by a fold, fading. -->
            <span class="tray" class:fading={cell.folding} aria-hidden="true"></span>
          {/if}
          <span class="ico">
            {#if cell.open}<FolderOpen />{:else}<Folder />{/if}
          </span>
          {#if app.renaming === cell.id}
            <!-- The hidden copy of the text gives the input its width. -->
            <span class="edit">
              <span class="sizer" aria-hidden="true">{nameText || ' '}</span>
              <input
                class="rename"
                size="1"
                spellcheck="false"
                autocomplete="off"
                autocorrect="off"
                autocapitalize="off"
                aria-label="Group name"
                bind:value={nameText}
                use:selectAll
                onkeydown={onNameKey}
                onblur={() => {
                  // Not the blur of an input that went as another group's name came up.
                  if (app.renaming === cell.id) endRename();
                }}
              />
            </span>
          {:else}
            <span class="name ellipsis">{cell.group.name}</span>
            {#if cell.group.tabs.length > 0}
              <!-- Tucked away while the group is open: its tabs are there to be counted. -->
              <span class="count" class:gone={cell.open} aria-hidden={cell.open}>{cell.group.tabs.length}</span>
            {/if}
          {/if}
        {:else}
          {@const repo = cell.tab.repo}
          <span class="label">
            <span class="ico">
              {#if !repo}
                <SquareDashed />
              {:else if repo.info.isLinkedWorktree}
                <GitFork />
              {:else}
                <GitBranch />
              {/if}
              {#if repo?.status?.operation}<span class="op"></span>{/if}
            </span>
            <span class="name ellipsis">{repo ? repo.info.name : NEW_TAB}</span>
          </span>
          <button
            class="close"
            title={active ? 'Close ⌘W' : 'Close'}
            onpointerdown={(e) => {
              if (e.button === 0) e.stopPropagation();
            }}
            onclick={() => app.closeTab(cell.id)}
          >
            <X size={12} />
          </button>
          {#if active && progress}<span class="progress" aria-hidden="true"></span>{/if}
        {/if}
      </div>
    {/each}
  </div>

  <button
    class="icon-btn"
    class:on={adding}
    title="New tab or group"
    aria-haspopup="menu"
    aria-expanded={adding}
    onclick={toggleAdd}
  >
    <Plus />
  </button>

  <div class="fill" data-tauri-drag-region></div>

  <div class="actions" data-tauri-drag-region>
    <UpdateButton />
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
    --bar-h: max(var(--tabbar-h), calc(var(--tabbar-h) / var(--zoom, 1)));
    /* What is left under a cell's row of icon and name, which is centered in the bar's content box
       (the bar's height less its bottom rule): the cell goes on down through it, to that rule. */
    --cell-foot: calc((var(--bar-h) - 1px - var(--tab-h)) / 2);
    height: var(--bar-h);
    padding: 0 8px 0 calc(78px / var(--zoom, 1));
    background: var(--bg-0);
    border-bottom: 1px solid var(--border);
  }
  /* Fullscreen hides the lights: plain zoomed metrics. */
  .tabbar.fullscreen {
    --bar-h: var(--tabbar-h);
    padding-left: 8px;
  }

  .strip {
    position: relative;
    flex: 0 1 auto;
    min-width: 0;
    height: 100%;
    display: flex;
    /* The cells stand on the bar's bottom edge. */
    align-items: flex-end;
    gap: var(--tab-gap);
    overflow-x: auto;
    overflow-y: hidden;
  }
  .strip::-webkit-scrollbar {
    display: none;
  }

  /* One cell of the row: a tab, or a group. Its row of icon and name is --tab-h high and centered in
     the bar, like everything else there; the cell itself goes on under that row (--cell-foot), down
     to the bar's bottom rule, and its top corners are rounded. So what fills a cell — the active tab,
     a group a tab is dragged into — has the shape of a browser's tab: clear of the bar's top, flush
     with its bottom edge. */
  .cell {
    /* The surface under the cell's content (an operation's dot is ringed in it), and the hairline
       after the cell. */
    --cell-bg: var(--bg-0);
    --cell-sep: var(--bg-tab-active);
    position: relative;
    flex: 0 1 auto;
    display: flex;
    align-items: center;
    gap: var(--tab-content-gap);
    min-width: 96px;
    max-width: 220px;
    height: calc(var(--tab-h) + var(--cell-foot));
    /* Balance the close button, its gap and its inset so the icon and name center on the tab. */
    padding: 0 var(--sp-1) var(--cell-foot) calc(var(--sp-1) + var(--tab-close-size) + var(--tab-content-gap));
    border-radius: var(--radius-sm) var(--radius-sm) 0 0;
    color: var(--text-2);
    font-size: var(--fs-tab);
    transition:
      transform 120ms ease-out,
      color var(--dur),
      background var(--dur);
  }
  .cell:hover {
    color: var(--text-1);
  }

  /* A group has no close button to balance. */
  .chip {
    min-width: 56px;
    max-width: 200px;
    padding: 0 10px var(--cell-foot);
  }
  /* Open, it names the tray its tabs sit on. */
  .chip.open {
    color: var(--text-1);
  }
  .chip.editing {
    flex: none;
    max-width: none;
  }

  .cell.active {
    --cell-bg: var(--bg-tab-active);
    background: var(--cell-bg);
    color: var(--text-0);
  }
  /* A tab of the open group, on the tray. */
  .cell.in-tray {
    --cell-bg: var(--bg-tray);
    --cell-sep: var(--bg-tray-active);
  }
  .cell.in-tray.active {
    --cell-bg: var(--bg-tray-active);
  }
  /* The open group's cell never has a fill of its own, the active cell (no tab shows) or not: the
     tray is its ground. */
  .cell.chip.open {
    background: none;
  }
  .cell.dragging {
    z-index: 1;
    transition: none;
  }
  .strip.dragging .cell {
    pointer-events: none;
  }
  /* A closed group the dragged tab would go into, and that tab over it (the group shows through).
     Outlined at the top and the sides: the bar's bottom rule closes it. */
  .cell.drop {
    background: var(--accent-dim);
    color: var(--text-0);
    box-shadow:
      inset 0 1px var(--border-strong),
      inset 1px 0 var(--border-strong),
      inset -1px 0 var(--border-strong);
  }
  .cell.into {
    opacity: 0.5;
  }

  /* A tab whose group unfolds, or folds away: the script leads its width from nothing, or to it,
     and the cell's edge passes over what it holds. */
  .cell.fold {
    overflow: hidden;
  }
  /* ...and one whose group closed, as the row has it once it is folded away, until it leaves the
     row: no room — the gap before it taken back —, nothing to see. */
  .cell.leaving {
    min-width: 0;
    max-width: 0;
    margin-left: calc(-1 * var(--tab-gap));
    padding-left: 0;
    padding-right: 0;
    opacity: 0;
  }

  /* The open group's tray: one box under the group's cell and its tabs, reaching --tray-pad past
     them at the sides and above, and with them down to the bar's bottom edge. It is the group's
     cell's own, so it moves with that cell and stays while one of the tabs is dragged along it; the
     script sets --tray-w, the width of those cells with the gaps between them and with the margins
     at the tray's ends, and --tray-l, the first of those margins (they are the tray's reach at the
     sides: --tray-pad, less while a group folds). Under everything in the bar: a resting cell is no
     stacking context, so the tray is painted with the bar's, right over the bar's background. A
     cell that is dragged or slides is one: the tray is then painted with that cell, still under
     what the cell holds and under the tabs, which follow it.
     The margins keep the cells outside the usual gap away from the tray. */
  .tray {
    position: absolute;
    z-index: -1;
    top: calc(-1 * var(--tray-pad));
    bottom: 0;
    left: calc(-1 * var(--tray-l, 0px));
    width: var(--tray-w, 0px);
    border-radius: var(--radius) var(--radius) 0 0;
    background: var(--bg-tray);
    pointer-events: none;
  }
  /* That of a group that closed: gone, once the script has led it there. */
  .tray.fading {
    opacity: 0;
  }
  .tray-start {
    margin-left: var(--tray-pad);
  }
  .tray-end {
    margin-right: var(--tray-pad);
  }

  /* Shared underline of a run of tabs of one repository (it and its worktrees), along the bar's
     bottom edge, where the cells end; right bridges the strip gap to the next tab of the run. The
     progress line comes later in the tab and draws on top. */
  .cell.run::before {
    content: '';
    position: absolute;
    left: 0;
    right: calc(-1 * var(--tab-gap));
    bottom: 0;
    height: 2px;
    background: var(--border-strong);
    pointer-events: none;
  }
  .cell.run-end::before {
    right: 0;
  }

  /* Hairline between adjacent cells, centered in the strip gap and on the cells' row of icon and
     name; none across an end of the open group's tray, between the group's name and its first tab,
     beside the active cell (its own edges part it from its neighbors), after the last cell or on
     the cell being dragged. */
  .cell.sep:not(.dragging)::after {
    content: '';
    position: absolute;
    top: 6px;
    bottom: calc(6px + var(--cell-foot));
    right: calc(-1 * (var(--tab-gap) + 1px) / 2);
    width: 1px;
    background: var(--cell-sep);
    pointer-events: none;
    transition: opacity var(--dur);
  }
  /* Nor on a tab that folds, whose edge would cut it off: it fades in once the tab is whole. */
  .cell.sep.fold::after {
    opacity: 0;
  }

  .label {
    flex: 1 1 auto;
    min-width: 0;
    display: flex;
    align-items: center;
    justify-content: center;
    gap: var(--tab-content-gap);
  }

  .name {
    flex: 0 1 auto;
    font-weight: 600;
    letter-spacing: var(--tab-letter-spacing);
  }

  /* The icon of the cell's kind: a step under the name. */
  .ico {
    position: relative;
    display: inline-flex;
    flex: none;
    color: var(--text-2);
    transition: color var(--dur);
  }
  .cell:hover .ico,
  .cell.active .ico,
  .cell.drop .ico,
  .chip.open .ico {
    color: var(--text-1);
  }

  /* An operation in progress, at the corner of the tab's icon; ringed in the surface under it. */
  .op {
    position: absolute;
    top: -2px;
    right: -3px;
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: var(--warn);
    box-shadow: 0 0 0 1.5px var(--cell-bg);
  }

  /* How many tabs a closed group holds. */
  .count {
    flex: none;
    overflow: hidden;
    font-family: var(--font-mono);
    font-size: var(--fs-xs);
    font-variant-numeric: tabular-nums;
    color: var(--text-2);
  }
  /* Tucked away while the group is open: no room beside the name — the gap before it taken back —,
     nothing to see. The script leads it there and back with the fold. */
  .count.gone {
    max-width: 0;
    margin-left: calc(-1 * var(--tab-content-gap));
    opacity: 0;
  }

  /* A group's name being typed: the hidden copy of the text sizes the input over it (the input's
     own `size` is 1, so that it asks for no width itself). */
  .edit {
    display: inline-grid;
    align-items: center;
    flex: none;
    min-width: 56px;
  }
  .edit > * {
    grid-area: 1 / 1;
    min-width: 0;
    font-weight: 600;
    letter-spacing: var(--tab-letter-spacing);
  }
  .sizer {
    visibility: hidden;
    /* The input's padding and border, and room for the caret. */
    padding: 0 6px;
    white-space: pre;
  }
  .rename {
    width: 100%;
    height: 20px;
    padding: 0 4px;
    border: 1px solid var(--focus-ring);
    border-radius: var(--radius-xs);
    background: var(--bg-input);
    color: var(--text-0);
    outline: none;
  }

  .close {
    flex: none;
    display: grid;
    place-items: center;
    width: var(--tab-close-size);
    height: var(--tab-close-size);
    border-radius: var(--radius-sm);
    color: var(--text-2);
    visibility: hidden;
  }
  .cell:hover .close {
    visibility: visible;
  }
  .close:hover {
    background: var(--bg-hover);
    color: var(--text-0);
  }
  /* On the tray: ink over its tint (the neutral fill would be a gray patch there). */
  .in-tray .close:hover {
    background: var(--accent-dim);
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
