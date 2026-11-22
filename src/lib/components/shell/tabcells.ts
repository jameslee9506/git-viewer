// The tab bar as a row of cells, and where a dragged cell belongs. Pure: TabBar passes the row the
// bar shows and the slots it measured.
import { isGroup, type Group, type Item, type Place, type Tab } from '../../state/app.svelte';

/** A tab's part of a run of adjacent tabs of one repository (the repo and its worktrees). */
export type RunPart = 'start' | 'mid' | 'end';

interface CellBase {
  /** The id of its tab or group. */
  id: number;
  /** Another cell follows it: at the top level a hairline stands between the two. */
  sep: boolean;
}

export interface TabCell extends CellBase {
  kind: 'tab';
  tab: Tab;
  run: RunPart | null;
}

/** A group in the top-level row: one cell, whatever it holds. */
export interface GroupCell extends CellBase {
  kind: 'group';
  group: Group;
}

export type Cell = TabCell | GroupCell;

/**
 * The cells of the row the bar shows, one per item: the top-level items (tabs and groups), or the
 * tabs of the group that is open.
 */
export function layoutCells(row: readonly Item[]): Cell[] {
  const cells = row.map(
    (it, i): Cell =>
      isGroup(it)
        ? { kind: 'group', id: it.id, group: it, sep: i < row.length - 1 }
        : { kind: 'tab', id: it.id, tab: it, run: null, sep: i < row.length - 1 },
  );
  /** The repository of the tab in `c`. */
  const dir = (c: Cell | undefined): string | undefined =>
    c?.kind === 'tab' ? c.tab.repo?.info.commonDir : undefined;
  for (let i = 0; i < cells.length; i++) {
    const c = cells[i];
    if (c.kind !== 'tab') continue;
    const key = dir(c);
    if (!key) continue;
    const before = dir(cells[i - 1]) === key;
    const after = dir(cells[i + 1]) === key;
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

/** The share of a group's cell, at either end, where a dragged tab passes it instead of going into it. */
const PASS = 0.2;

/**
 * Where the dragged tab `id` belongs in its row — the tabs of the group `group`, or the top level
 * (null) — with the pointer at `x`: past every cell whose middle the pointer has passed. A group
 * with the pointer over its middle part is `into`: the tab goes into it when released there, and
 * until then keeps the side of it that it is on.
 *
 * The pointer decides, not the tab's edges. A cell the tab passes moves the other way, away from
 * the pointer, so an answer never flips back on the next frame.
 */
export function placeTab(
  slots: Slot[],
  id: number,
  x: number,
  group: number | null,
): { to: Place; into: number | null } {
  let index = 0;
  let into: number | null = null;
  /** The dragged tab's own cell lies behind. */
  let passed = false;
  for (const s of slots) {
    const c = s.cell;
    if (c.kind === 'tab') {
      if (c.id === id) passed = true;
      else if (middle(s) < x) index++;
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
  return { to: { group, index }, into };
}

/**
 * The top-level position the dragged group `id` belongs at, drawn from `left` to `right`: a cell
 * after it is passed once the group's right edge is over that cell's middle, a cell before it once
 * its left edge is. Passed cells move by the group's whole width, away from the edge that passed
 * them.
 */
export function placeGroup(slots: Slot[], id: number, left: number, right: number): number {
  let index = 0;
  /** The dragged group's own cell lies behind. */
  let passed = false;
  for (const s of slots) {
    if (s.cell.id === id) {
      passed = true;
      continue;
    }
    const mid = middle(s);
    if (passed ? right > mid : left >= mid) index++;
  }
  return index;
}
