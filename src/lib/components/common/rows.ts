// Row geometry of a virtual list whose rows share one height except a few taller ones (the sidebar's
// section headers). Positions are virtual px: row 0 starts at 0, row i at the sum of the heights
// before it. Every lookup is O(log tall) — the fixed-height fast path when there are no tall rows.

export interface RowMetrics {
  /** Height of an ordinary row. */
  rowHeight: number;
  /** Indices of the taller rows, ascending (indices past the list end are ignored). */
  tall: ArrayLike<number>;
  /** How much taller those rows are (0: every row is `rowHeight`). */
  extra: number;
}

const NONE: readonly number[] = [];

export function uniformRows(rowHeight: number): RowMetrics {
  return { rowHeight, tall: NONE, extra: 0 };
}

function uniform(m: RowMetrics): boolean {
  return m.extra === 0 || m.tall.length === 0;
}

/** Number of tall rows before row i. */
function tallBefore(m: RowMetrics, i: number): number {
  const t = m.tall;
  let lo = 0;
  let hi = t.length;
  while (lo < hi) {
    const mid = (lo + hi) >> 1;
    if (t[mid] < i) lo = mid + 1;
    else hi = mid;
  }
  return lo;
}

export function isTall(m: RowMetrics, i: number): boolean {
  if (uniform(m)) return false;
  const k = tallBefore(m, i);
  return k < m.tall.length && m.tall[k] === i;
}

/** Top of row i (also the height of the i rows before it). */
export function rowTop(m: RowMetrics, i: number): number {
  return i * m.rowHeight + (uniform(m) ? 0 : tallBefore(m, i) * m.extra);
}

export function rowHeightAt(m: RowMetrics, i: number): number {
  return m.rowHeight + (isTall(m, i) ? m.extra : 0);
}

/** The row at virtual position y: the last of `count` rows starting at or above y (0 for y < 0). */
export function rowAt(m: RowMetrics, y: number, count: number): number {
  if (count <= 0) return 0;
  if (uniform(m)) return Math.max(0, Math.min(count - 1, Math.floor(y / m.rowHeight)));
  let lo = 0;
  let hi = count - 1;
  while (lo < hi) {
    const mid = (lo + hi + 1) >> 1;
    if (rowTop(m, mid) <= y) lo = mid;
    else hi = mid - 1;
  }
  return lo;
}

/** The first of `count` rows starting at or below y (`count` when none): the exclusive end of the rows above y. */
export function rowFrom(m: RowMetrics, y: number, count: number): number {
  if (count <= 0) return 0;
  if (uniform(m)) return Math.max(0, Math.min(count, Math.ceil(y / m.rowHeight)));
  let lo = 0;
  let hi = count;
  while (lo < hi) {
    const mid = (lo + hi) >> 1;
    if (rowTop(m, mid) >= y) hi = mid;
    else lo = mid + 1;
  }
  return lo;
}
