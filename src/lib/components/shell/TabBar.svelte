<!--
  Title bar: the tabs — repositories, new tabs and groups of them — and the global toggles. The bar
  shows one row at a time: the top level, where a group is one cell, or the tabs of the group that
  was opened, in place of the top level. Every tab starts with the icon of its kind, and the active
  one is filled like a browser's tab: clear of the bar's top, its top corners rounded, standing on
  the bar's bottom edge, with what it holds centered in that shape. A group's row stands on a tinted
  tray: the group's name first — the way back to the top level —, then its tabs, each a button of
  that shape in the bar's own ground.
  A row too long for the bar scrolls. A pointer drag reorders the row and drops a tab onto a group;
  middle-click closes a tab; double-click names a group. The `+` after the row opens a menu under
  it: a new tab, or a new group.
  Empty bar space drags the window (data-tauri-drag-region); the traffic lights sit in the left
  padding (78pt, 8px in fullscreen). The webview zoom scales CSS px but not the native lights, so
  that padding and the bar's minimum height (its 100% height) are held in points at every zoom.
-->
<script lang="ts">
  import { onMount, tick, untrack } from 'svelte';
  import { flip } from 'svelte/animate';
  import { getCurrentWindow } from '@tauri-apps/api/window';
  import { revealItemInDir } from '@tauri-apps/plugin-opener';
  import ChevronLeft from '@lucide/svelte/icons/chevron-left';
  import Copy from '@lucide/svelte/icons/copy';
  import Folder from '@lucide/svelte/icons/folder';
  import FolderInput from '@lucide/svelte/icons/folder-input';
  import FolderOpen from '@lucide/svelte/icons/folder-open';
  import FolderOutput from '@lucide/svelte/icons/folder-output';
  import FolderPlus from '@lucide/svelte/icons/folder-plus';
  import FolderX from '@lucide/svelte/icons/folder-x';
  import GitBranch from '@lucide/svelte/icons/git-branch';
  import GitFork from '@lucide/svelte/icons/git-fork';
  import LayoutList from '@lucide/svelte/icons/layout-list';
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
  import { basename, copyText } from '../../util/format';
  import { isComposing, isEditable } from '../../util/keys';
  import Spinner from '../common/Spinner.svelte';
  import SettingsPopover, { clampZoom } from './SettingsPopover.svelte';
  import { layoutCells, placeGroup, placeTab, type Cell, type Slot, type TabCell } from './tabcells';
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

  interface Press {
    /** The pressed cell: a tab's or a group's id. */
    id: number;
    /** A group: it moves among the top-level items. */
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
  /** The group the dragged tab is over: released there, it goes into it. */
  let dropInto = $state<number | null>(null);
  /** The text of the group name being typed (app.renaming). */
  let nameText = $state('');

  let press: Press | null = null;
  let frame = 0;
  let middleId: number | null = null;
  /**
   * The last click in the row: where and when, the group it was on (if one: its cell, or its name
   * before its tabs), and whether it had the bar show another row. That puts other cells under the
   * pointer, so the second click of a double-click meets whatever came there: double-clicks are told
   * by the bar (onBarPointerDown), not by `dblclick`.
   */
  let lastClick: { at: number; x: number; y: number; group: number | null; relaid: boolean } | null = null;
  /** The cell a drag just let go of: it settles into its slot by its own transition (see cellFlip). */
  let settling: number | null = null;

  const activeRepo = $derived(app.active?.repo ?? null);
  const busy = $derived(!!activeRepo && (activeRepo.graph.loading || !activeRepo.ready));
  /** Webview zoom as App applies it (setZoom); `.tabbar` divides its traffic-light clearance by it. */
  const zoom = $derived(clampZoom(settings.zoom));

  /** The group whose tabs the bar shows; null while it shows the top level. */
  const group = $derived(app.openGroup);
  const cells = $derived(layoutCells(app.stops));
  /**
   * The cell of what shows: the active tab's — or, at the top level, that of the group that holds
   * the active tab.
   */
  const currentId = $derived(group ? app.activeId : (app.activeGroup?.id ?? app.activeId));

  $effect(() => {
    if (!busy) {
      progress = false;
      return;
    }
    const timer = setTimeout(() => (progress = true), PROGRESS_DELAY_MS);
    return () => clearTimeout(timer);
  });

  /** The row the strip was last scrolled for. */
  let rowShown: number | null | undefined;

  // Keep what shows in view whenever the row or the active tab changes: the active tab, or the cell
  // of the group that holds it. Another row starts from its beginning. Not while a drag is moving
  // cells around.
  $effect(() => {
    const s = strip;
    const id = currentId;
    const row = app.openGroupId;
    void cells;
    if (!s || dragId !== null) return;
    if (row !== rowShown) {
      rowShown = row;
      s.scrollLeft = 0;
    }
    if (id == null) return;
    const el = s.querySelector<HTMLElement>(`[data-cell="${id}"]`);
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

  function isActive(cell: Cell | undefined): boolean {
    return cell !== undefined && cell.id === currentId;
  }

  function opTitle(op: RepoOperation): string {
    return op.step != null && op.total != null ? `${op.kind} ${op.step}/${op.total}` : op.kind;
  }

  /** Tooltip: a tab's path (a clone's URL) and its operation in progress; what a group holds. */
  function cellTitle(cell: Cell): string | undefined {
    const lines: string[] = [];
    if (cell.kind === 'group') {
      for (const t of cell.group.tabs) lines.push(t.repo?.info.name ?? NEW_TAB);
    } else {
      const repo = cell.tab.repo;
      const clone = repo ? null : app.cloneFor(cell.tab.id);
      if (repo) lines.push(repo.info.path);
      else if (clone) lines.push(clone.url);
      const op = repo?.status?.operation;
      if (op) lines.push(opTitle(op));
    }
    return lines.length > 0 ? lines.join('\n') : undefined;
  }

  // ───────────── drag ─────────────

  /**
   * The bar shows another row: its cells take their places at once. A tab that came along from the
   * other row does not slide to its new place.
   */
  let relaid = false;
  $effect.pre(() => {
    void app.openGroupId;
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
   * on: on a group it names the group — in the group's own row, where its name stands before its
   * tabs —, and after a click that had the bar show another row it is dropped: that click put
   * another cell, a button or the bar's bare ground (where a double-click zooms the window) under
   * the pointer.
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
    const id = last.group;
    // After this press: a mousedown in the bar ends a name being typed (onBarMouseDown).
    if (id != null) {
      setTimeout(() => {
        if (!app.items.some((it) => isGroup(it) && it.id === id)) return;
        // The first click left the group (it was on its name): back into it.
        if (app.openGroupId !== id) app.enter(id);
        app.renaming = id;
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
    // A tab is active at once. A press on a group opens it on release: until then it may turn into a
    // drag, which must not have the bar show another row under the pointer.
    let click: (() => void) | null = null;
    if (cell.kind === 'group') click = () => app.enter(cell.id);
    else app.activate(cell.id);
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

  /** A click on the open group's name, before its tabs: back to the top level. */
  function onHeadClick(e: MouseEvent, id: number) {
    // Not while the name is typed (a press beside the input has ended that by now: this is one that
    // began in it, like a selection drawn out over the icon), nor a context click.
    if (app.renaming === id || isEditable(e.target) || e.button !== 0 || e.ctrlKey) return;
    app.leave();
    lastClick = { at: e.timeStamp, x: e.clientX, y: e.clientY, group: id, relaid: true };
  }

  function onWindowBlur() {
    endPress();
  }

  /**
   * The press is over. A dragged tab stays where the drag put it — in the group `into` when it was
   * released over one — and a tab of the same repository beside it gives way.
   */
  function endPress(into: number | null = null) {
    const p = press;
    const dragged = p !== null && dragId === p.id;
    press = null;
    if (frame) cancelAnimationFrame(frame);
    frame = 0;
    dragId = null;
    dragDx = 0;
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
    const els = new Map<string, HTMLElement>();
    for (const el of s.querySelectorAll<HTMLElement>('[data-cell]')) els.set(el.dataset.cell ?? '', el);
    const slots: Slot[] = [];
    for (const cell of cells) {
      const el = els.get(String(cell.id));
      if (el) slots.push({ cell, left: base + el.offsetLeft, right: base + el.offsetLeft + el.offsetWidth });
    }
    return slots;
  }

  /** Where the dragged cell at `me` is drawn: under the pointer, inside the row. */
  function drawn(slots: Slot[], me: number, p: Press): { left: number; right: number } {
    const slot = slots[me];
    const width = slot.right - slot.left;
    const min = slots[0].left;
    const max = slots[slots.length - 1].right - width;
    const left = Math.max(min, Math.min(max, p.x - p.grab * width));
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
    // A move lays the row out anew, which may ask for one more (the cells differ in width): a few
    // rounds settle it while the pointer rests.
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
        ({ to, into } = placeTab(slots, p.id, p.x, at.group));
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

  /** The name input: one at a time, before the open group's tabs or in a group's cell. */
  let nameInput: HTMLInputElement | undefined = $state();

  // The input has the keyboard focus, the name in it selected: typing replaces it. Also when the
  // input stays and the next group's name comes up in it (a new group made while one is typed).
  $effect(() => {
    if (app.renaming == null) return;
    nameInput?.focus();
    nameInput?.select();
  });

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
    if (cell.kind === 'group') groupMenu(e, cell.group);
    else tabMenu(e, cell);
  }

  function tabMenu(e: MouseEvent, cell: TabCell) {
    const tab = cell.tab;
    /** The group whose row the tab is in. */
    const home = group;
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

  /** The menu of a group: on its cell at the top level, and on its name before its tabs. */
  function groupMenu(e: MouseEvent, g: Group) {
    if (isEditable(e.target)) return;
    const n = g.tabs.length;
    showContextMenu(e, [
      { label: 'Rename', icon: Pencil, action: () => (app.renaming = g.id) },
      { label: 'Ungroup', icon: Ungroup, disabled: n === 0, action: () => app.ungroup(g.id) },
      { separator: true },
      // Deleting its tabs with it asks first.
      { label: n > 0 ? 'Delete group…' : 'Delete group', icon: FolderX, action: () => void app.deleteGroup(g.id) },
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

<!-- A group's name being typed: the hidden copy of the text gives the input its width. -->
{#snippet nameField(id: number)}
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
      bind:this={nameInput}
      onkeydown={onNameKey}
      onblur={() => {
        // Not the blur of an input that went as another group's name came up.
        if (app.renaming === id) endRename();
      }}
    />
  </span>
{/snippet}

<!-- svelte-ignore a11y_no_static_element_interactions -->
<header
  class="tabbar"
  class:fullscreen
  style:--zoom={zoom}
  data-tauri-drag-region
  onpointerdowncapture={onBarPointerDown}
  onmousedown={onBarMouseDown}
>
  <!-- The row the bar shows: the top level — or, on its tray, the open group's name and its tabs. -->
  <div class="row" class:grouped={group !== null}>
    {#if group}
      <!-- The way back to the top level. A pointer's only: ⌥⌘↑ does the same from the keyboard. -->
      <!-- svelte-ignore a11y_click_events_have_key_events -->
      <div
        class="head"
        class:editing={app.renaming === group.id}
        role="button"
        tabindex="-1"
        title={app.renaming === group.id ? undefined : 'Back ⌥⌘↑'}
        onclick={(e) => onHeadClick(e, group.id)}
        oncontextmenu={(e) => groupMenu(e, group)}
      >
        <span class="ico"><ChevronLeft /></span>
        {#if app.renaming === group.id}
          {@render nameField(group.id)}
        {:else}
          <span class="name ellipsis">{group.name}</span>
        {/if}
      </div>
    {/if}
    <div
      class="strip"
      class:dragging={dragId !== null}
      class:bare={cells.length === 0}
      bind:this={strip}
      role="tablist"
      onwheel={onWheel}
    >
      {#each cells as cell, i (cell.id)}
        {@const moving = cell.id === dragId}
        <!-- A group is the active cell while the tab that shows is one of its tabs. -->
        {@const active = isActive(cell)}
        <div
          class="cell"
          class:chip={cell.kind === 'group'}
          class:editing={app.renaming === cell.id}
          class:active
          class:sep={group === null && cell.sep && !active && !isActive(cells[i + 1])}
          class:run={cell.kind === 'tab' && cell.run !== null}
          class:run-end={cell.kind === 'tab' && cell.run === 'end'}
          class:dragging={moving}
          class:into={moving && dropInto !== null}
          class:drop={dropInto === cell.id}
          role="tab"
          tabindex="-1"
          aria-selected={active}
          data-cell={cell.id}
          title={cellTitle(cell)}
          style:transform={moving ? `translateX(${dragDx}px)` : null}
          animate:cellFlip
          onpointerdown={(e) => onCellPointerDown(e, cell)}
          onpointerup={(e) => onCellPointerUp(e, cell)}
          oncontextmenu={(e) => onCellContextMenu(e, cell)}
        >
          {#if cell.kind === 'group'}
            <span class="ico">
              {#if active}<FolderOpen />{:else}<Folder />{/if}
            </span>
            {#if app.renaming === cell.id}
              {@render nameField(cell.id)}
            {:else}
              <span class="name ellipsis">{cell.group.name}</span>
              {#if cell.group.tabs.length > 0}
                <span class="count">{cell.group.tabs.length}</span>
              {/if}
            {/if}
          {:else}
            {@const repo = cell.tab.repo}
            {@const clone = repo ? null : app.cloneFor(cell.tab.id)}
            <span class="label">
              <span class="ico">
                {#if clone}
                  <Spinner size={14} />
                {:else if !repo}
                  <SquareDashed />
                {:else if repo.info.isLinkedWorktree}
                  <GitFork />
                {:else}
                  <GitBranch />
                {/if}
                {#if repo?.status?.operation}<span class="op"></span>{/if}
              </span>
              <span class="name ellipsis">{repo ? repo.info.name : clone ? basename(clone.dest) : NEW_TAB}</span>
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
  </div>

  <button
    class="icon-btn plus"
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
    <!-- The simple view: one branch's history as a list (a setting of the app, not of the tab). -->
    <button
      class="icon-btn"
      class:on={settings.simpleView}
      title="Simple view ⇧⌘L"
      aria-pressed={settings.simpleView}
      onclick={() => (settings.simpleView = !settings.simpleView)}
    >
      <LayoutList />
    </button>
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
    /* The bar's margin above its cells: what a cell --tab-h high, standing on the bar's bottom rule,
       leaves of the bar at 100% (5px). */
    --cell-top: calc(var(--tabbar-h) - 1px - var(--tab-h));
    /* A cell reaches from there down to that rule: --tab-h, and more where the bar is higher than
       --tabbar-h (zoomed out) — the margin above stays. */
    --cell-h: calc(var(--bar-h) - 1px - var(--cell-top));
    /* A tab's close button is centered in the square at the tab's end: as far from the tab's right
       edge as from its top and its bottom (7px). From --tab-h, not --cell-h: where the bar is
       higher, a cell gains room above and below what it holds, none at its sides. */
    --close-inset: calc((var(--tab-h) - var(--tab-close-size)) / 2);
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

  /* The row the bar shows stands on the bar's bottom edge: the top level's cells — or the open
     group's name and its tabs, on a tray. */
  .row {
    flex: 0 1 auto;
    min-width: 0;
    align-self: flex-end;
    display: flex;
    align-items: flex-end;
    gap: var(--tab-gap);
    height: var(--cell-h);
  }
  /* The tray of a group's row: one box in the bar's tint under the group's name and its tabs,
     reaching --tray-pad past them at the sides and above, and with them down to the bar's bottom
     edge. It is the row's own ground, so it is as wide as what the row shows: when the tabs are too
     many for the bar they scroll on it, and the name stays. */
  .row.grouped {
    height: calc(var(--cell-h) + var(--tray-pad));
    padding: var(--tray-pad) var(--tray-pad) 0;
    border-radius: var(--radius) var(--radius) 0 0;
    background: var(--bg-tray);
  }

  .strip {
    position: relative;
    flex: 0 1 auto;
    min-width: 0;
    height: var(--cell-h);
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
  /* Without a cell (a group that holds no tab) it takes no room: no gap after the group's name. */
  .strip.bare {
    display: none;
  }

  /* One cell of the row: a tab, or a group. It stands on the bar's bottom rule, --cell-top under the
     bar's top, and its top corners are rounded. So what fills a cell — the active tab, a tab of a
     group, a group a tab is dragged into — has the shape of a browser's tab: clear of the bar's top,
     flush with its bottom edge. What a cell holds is centered in that shape, between its top and its
     bottom: the icon, the name, a tab's close button. */
  .cell {
    /* The surface under the cell's content (an operation's dot is ringed in it). */
    --cell-bg: var(--bg-0);
    position: relative;
    flex: 0 1 auto;
    display: flex;
    align-items: center;
    gap: var(--tab-content-gap);
    min-width: 102px;
    max-width: 226px;
    height: var(--cell-h);
    /* The left inset balances the close button, its gap and its inset, so the icon and name center
       on the tab from side to side too. */
    padding: 0 var(--close-inset) 0 calc(var(--close-inset) + var(--tab-close-size) + var(--tab-content-gap));
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
    padding: 0 10px;
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
  /* The group that holds the tab that shows: in the tint its row stands on. */
  .cell.chip.active {
    --cell-bg: var(--bg-tray);
  }
  /* A tab of a group: a button of its own on the tray, in the bar's ground — the tray shows between
     the tabs and parts them. The active one is filled as at the top level. */
  .row.grouped .cell {
    background: var(--cell-bg);
  }
  .cell.dragging {
    z-index: 1;
    transition: none;
  }
  .strip.dragging .cell {
    pointer-events: none;
  }
  /* A group the dragged tab would go into, and that tab over it (the group shows through).
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

  /* The open group's name, before its tabs: a press on it leads back to the top level. No fill of
     its own: the tray is its ground. As high as the cells, and what it holds on their middle. */
  .head {
    flex: none;
    display: flex;
    align-items: center;
    gap: 4px;
    max-width: 200px;
    height: var(--cell-h);
    padding: 0 8px 0 4px;
    border-radius: var(--radius-sm) var(--radius-sm) 0 0;
    color: var(--text-1);
    font-size: var(--fs-tab);
    transition:
      color var(--dur),
      background var(--dur);
  }
  /* Under the pointer: ink over the tint, in a cell's shape. */
  .head:hover {
    background: var(--accent-dim);
    color: var(--text-0);
  }
  .head.editing {
    max-width: none;
    background: none;
  }
  .head .ico {
    color: inherit;
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

  /* Hairline between adjacent cells of the top level, centered in the strip gap and, as tall as the
     icons, on their line: the middle of the cells; none beside the active cell (its own edges part
     it from its neighbors), after the last cell or on the cell being dragged. A group's tabs have
     none: each is a button of its own. */
  .cell.sep:not(.dragging)::after {
    content: '';
    position: absolute;
    top: calc(50% - 7px);
    height: 14px;
    right: calc(-1 * (var(--tab-gap) + 1px) / 2);
    width: 1px;
    background: var(--bg-tab-active);
    pointer-events: none;
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
  .cell.drop .ico {
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

  /* How many tabs a group holds. */
  .count {
    flex: none;
    font-family: var(--font-mono);
    font-size: var(--fs-xs);
    font-variant-numeric: tabular-nums;
    color: var(--text-2);
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

  /* The `+` after the row stays on the row's line: with the cells' margin above it, the bar centers
     it between the cells' top and their bottom. (What stands at the bar's other end is not of the
     row: centered in the bar.) */
  .plus {
    margin-top: var(--cell-top);
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
