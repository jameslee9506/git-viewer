// The tab bar as a row of cells, and where a dragged cell belongs. Pure: TabBar passes the bar's items
// and the slots it measured.
import { isGroup, type Group, type Item, type Place, type Tab } from '../../state/app.svelte';

/** A tab's part of a run of adjacent tabs of one repository (the repo and its worktrees). */
export type RunPart = 'start' | 'mid' | 'end';
/**
 * A cell's part of a group's tray, which runs from the group's cell (`start`) to the last of its
 * tabs (`end`); `only` is the cell of a group without tabs.
 */
export type TrayPart = 'start' | 'mid' | 'end' | 'only';
/**
 * A tab's part of a fold: `in` while its group, opened a moment ago, unfolds; `out` once its group
 * closed, until it is folded away.
 */
export type FoldPart = 'in' | 'out';

interface CellBase {
  /** The id of its tab or group. */
  id: number;
  /** Its part of a tray — the open group's, or that of a group that is folding —; null outside one. */
  tray: TrayPart | null;
  /** A hairline separates it from the next cell. */
  sep: boolean;
}

export interface TabCell extends CellBase {
  kind: 'tab';
  tab: Tab;
  /** The group whose tray it is on: the open one, or one that is folding. Null at the top level. */
  group: Group | null;
  run: RunPart | null;
  fold: FoldPart | null;
}

export interface GroupCell extends CellBase {
  kind: 'group';
  group: Group;
  /** Its tabs follow it in the row. */
  open: boolean;
  /** Closed a moment ago: its tabs still follow it, folding away on what is left of its tray. */
  folding: boolean;
}

export type Cell = TabCell | GroupCell;

/**
 * A group that closed a moment ago and is folding: its id, and the tabs its tray showed when it
 * closed, in the tray's order.
 */
export interface Folding {
  id: number;
  tabs: readonly number[];
}

/**
 * The bar's row: every top-level item is a cell, and the tabs of the open group (`open`) follow their
 * group's cell, on its tray. Every cell keeps its size, whichever group is open.
 *
 * A change of the open group is animated, so for a moment the row holds more: the tabs of the groups
 * that just closed (`folding`) still follow their cells, as `out` parts, and those of the open group
 * are `in` parts while it unfolds (`unfolding`).
 */
export function layoutCells(
  items: Item[],
  open: number | null,
  folding: readonly Folding[] = [],
  unfolding = false,
): Cell[] {
  const cells: Cell[] = [];
  /** A tab's cell: at the top level, or on the tray of `group`. */
  const put = (tab: Tab, group: Group | null = null, tray: TrayPart | null = null, fold: FoldPart | null = null) =>
    cells.push({ kind: 'tab', id: tab.id, tab, group, run: null, fold, tray, sep: false });
  for (let i = 0; i < items.length; i++) {
    const it = items[i];
    if (!isGroup(it)) {
      put(it);
      continue;
    }
    const isOpen = it.id === open;
    const fold = isOpen ? undefined : folding.find((f) => f.id === it.id);
    cells.push({
      kind: 'group',
      id: it.id,
      group: it,
      open: isOpen,
      folding: !!fold,
      tray: isOpen || fold ? (it.tabs.length > 0 ? 'start' : 'only') : null,
      sep: false,
    });
    if (isOpen) {
      it.tabs.forEach((tab, k) => put(tab, it, k === it.tabs.length - 1 ? 'end' : 'mid', unfolding ? 'in' : null));
    } else if (fold) {
      // Its tabs stay where its tray had them. So does a tab that has just left the group to stand
      // right after it, at the top level: it keeps its place among them until they are folded away
      // (it is then the group's neighbor anyway). A tab the tray did not show comes last.
      const next = items[i + 1];
      const guest = next && !isGroup(next) && fold.tabs.includes(next.id) ? next : null;
      const place = (tab: Tab) => {
        const k = fold.tabs.indexOf(tab.id);
        return k < 0 ? fold.tabs.length : k;
      };
      const row = guest ? [...it.tabs, guest] : [...it.tabs];
      let left = it.tabs.length;
      for (const tab of row.sort((a, b) => place(a) - place(b))) {
        if (tab === guest) put(tab);
        else put(tab, it, --left === 0 ? 'end' : 'mid', 'out');
      }
      // The loop's next item is that tab: it has its cell.
      if (guest) i++;
    }
  }
  /** The repository of the tab in `c`, when `c` is a tab in the row of the tab `of`. */
  const dir = (c: Cell | undefined, of: TabCell): string | undefined =>
    c?.kind === 'tab' && c.group === of.group ? c.tab.repo?.info.commonDir : undefined;
  /** The part of `c` in the open group's tray. A group that is folding has its hairlines as a closed one. */
  const part = (c: Cell): TrayPart | null => ((c.kind === 'group' ? c.open : c.fold !== 'out') ? c.tray : null);
  for (let i = 0; i < cells.length; i++) {
    const c = cells[i];
    const next = cells[i + 1];
    // Not across an end of the open group's tray, nor on it between the group's name and its first tab.
    const here = part(c);
    const there = next ? part(next) : null;
    c.sep = !!next && (here === null || here === 'mid') && there !== 'start' && there !== 'only';
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
 * The pointer decides, not the tab's edges. An item the tab passes moves the other way, away from
 * the pointer, so an answer never flips back on the next frame.
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
