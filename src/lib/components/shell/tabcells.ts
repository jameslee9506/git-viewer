// The tab bar as a row of cells, and where a dragged cell belongs. Pure: TabBar passes the bar's items
// and the slots it measured.
import { isGroup, type Group, type Item, type Place, type Tab } from '../../state/app.svelte';

/** A tab's part of a run of adjacent tabs of one repository (the repo and its worktrees). */
export type RunPart = 'start' | 'mid' | 'end';
/** A cell's part of the outline around the open group. */
export type TrayPart = 'start' | 'mid' | 'end' | 'only';

interface CellBase {
  /** The id of its tab or group. */
  id: number;
  /** Icon only: what lies outside the open group. */
  compact: boolean;
  /** Its part of the open group's outline; null outside it. */
  tray: TrayPart | null;
  /** A hairline separates it from the next cell. */
  sep: boolean;
}

export interface TabCell extends CellBase {
  kind: 'tab';
  tab: Tab;
  /** The open group it is in; null at the top level. */
  group: Group | null;
  run: RunPart | null;
}

export interface GroupCell extends CellBase {
  kind: 'group';
  group: Group;
  /** Its tabs follow it in the row. */
  open: boolean;
}

export type Cell = TabCell | GroupCell;

/**
 * The bar's row: every top-level item is a cell, and the tabs of the open group (`open`) follow their
 * group's cell. While a group is open, every cell outside it is compact — but the group `wide`, whose
 * name is being typed.
 */
export function layoutCells(items: Item[], open: number | null, wide: number | null = null): Cell[] {
  const cells: Cell[] = [];
  const shut = open != null;
  for (const it of items) {
    if (!isGroup(it)) {
      cells.push({ kind: 'tab', id: it.id, tab: it, group: null, run: null, compact: shut, tray: null, sep: false });
      continue;
    }
    const isOpen = it.id === open;
    cells.push({
      kind: 'group',
      id: it.id,
      group: it,
      open: isOpen,
      compact: shut && !isOpen && it.id !== wide,
      tray: isOpen ? (it.tabs.length > 0 ? 'start' : 'only') : null,
      sep: false,
    });
    if (!isOpen) continue;
    it.tabs.forEach((tab, i) => {
      const tray = i === it.tabs.length - 1 ? 'end' : 'mid';
      cells.push({ kind: 'tab', id: tab.id, tab, group: it, run: null, compact: false, tray, sep: false });
    });
  }
  /** The repository of the tab in `c`, when `c` is a tab in the row of the tab `of`. */
  const dir = (c: Cell | undefined, of: TabCell): string | undefined =>
    c?.kind === 'tab' && c.group === of.group ? c.tab.repo?.info.commonDir : undefined;
  for (let i = 0; i < cells.length; i++) {
    const c = cells[i];
    const next = cells[i + 1];
    // Not across the outline of the open group.
    c.sep = !!next && c.tray !== 'end' && c.tray !== 'only' && next.tray !== 'start' && next.tray !== 'only';
    if (c.kind !== 'tab') continue;
    const key = c.tab.repo?.info.commonDir;
    if (!key) continue;
    const before = dir(cells[i - 1], c) === key;
    const after = dir(next, c) === key;
    c.run = before && after ? 'mid' : before ? 'end' : after ? 'start' : null;
  }
  return cells;
}

/** A cell and where layout puts it along the bar, in px (a drag's transform left aside). */
export interface Slot {
  cell: Cell;
  left: number;
  right: number;
}

const middle = (s: { left: number; right: number }) => (s.left + s.right) / 2;

/** The share of a closed group's cell, at either end, where a dragged tab passes it instead of going into it. */
const PASS = 0.2;

/**
 * Where the dragged tab `id` belongs with the pointer at `x`.
 *
 * Inside the open group from the middle of the group's cell to the end of its last tab — its own
 * slot included while it is that last tab, so that it leaves only once the pointer is past it — at
 * the position among the group's tabs the pointer has reached. Elsewhere at the top level, past
 * every item whose middle the pointer has passed. A closed group with the pointer over its middle
 * part is `into`: the tab goes into it when released there, and until then keeps the side of it
 * that it is on.
 *
 * The pointer decides, not the tab's edges: a tab changes width as it enters or leaves the open
 * group. An item the tab passes moves the other way, away from the pointer, so an answer never
 * flips back on the next frame.
 */
export function placeTab(slots: Slot[], id: number, x: number): { to: Place; into: number | null } {
  const tray = slots.find((s) => s.cell.kind === 'group' && s.cell.open);
  if (tray) {
    const inner = slots.filter((s) => s.cell.kind === 'tab' && s.cell.group?.id === tray.cell.id);
    const end = inner.length > 0 ? inner[inner.length - 1].right : tray.right;
    if (x >= middle(tray) && x <= end) {
      let index = 0;
      for (const s of inner) if (s.cell.id !== id && middle(s) < x) index++;
      return { to: { group: tray.cell.id, index }, into: null };
    }
  }
  let index = 0;
  let into: number | null = null;
  /** The dragged tab's own cell lies behind (inside the open group, it follows that group's cell). */
  let passed = false;
  for (const s of slots) {
    const c = s.cell;
    if (c.kind === 'tab') {
      if (c.id === id) passed = true;
      else if (!c.group && middle(s) < x) index++;
    } else if (c.open) {
      // Not inside it (see above): at or past its middle is past its end.
      if (x >= middle(s)) index++;
    } else {
      const edge = (s.right - s.left) * PASS;
      if (x >= s.left + edge && x <= s.right - edge) {
        into = c.id;
        if (!passed) index++;
      } else if (middle(s) < x) {
        index++;
      }
    }
  }
  return { to: { group: null, index }, into };
}

/** The right end of the cell at `i` — of the last of its tabs, for an open group. */
export function unitEnd(slots: Slot[], i: number): number {
  const c = slots[i].cell;
  let right = slots[i].right;
  if (c.kind !== 'group' || !c.open) return right;
  for (let k = i + 1; k < slots.length; k++) {
    const n = slots[k].cell;
    if (n.kind !== 'tab' || n.group?.id !== c.id) break;
    right = slots[k].right;
  }
  return right;
}

/**
 * The top-level position the dragged group `id` belongs at, drawn from `left` to `right` (an open
 * group moves with its tabs, so it may be much wider than what it passes): an item after it is
 * passed once the group's right edge is over that item's middle, an item before it once its left
 * edge is. Passed items move by the group's whole width, away from the edge that passed them.
 */
export function placeGroup(slots: Slot[], id: number, left: number, right: number): number {
  let index = 0;
  /** The dragged group's own cell lies behind. */
  let passed = false;
  for (let i = 0; i < slots.length; i++) {
    const s = slots[i];
    const c = s.cell;
    if (c.id === id) {
      passed = true;
      continue;
    }
    // A group's tabs belong to its cell.
    if (c.kind === 'tab' && c.group) continue;
    const mid = (s.left + unitEnd(slots, i)) / 2;
    if (passed ? right > mid : left >= mid) index++;
  }
  return index;
}
