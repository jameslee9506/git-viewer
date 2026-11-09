// Commit graph geometry: lane/row coordinates, the branch / tag cells'
// extent and the SVG path data for a window of rows, batched into one path per CSS class.
import { EDGE_DASHED, EDGE_FROM_NODE, EDGE_MERGE, EDGE_OPEN, EDGE_TO_NODE } from '../../types';
import type { GraphRow } from '../../types';
import { AVATAR_R } from '../../util/identicon';

/** Graph row height in px (matches --row-h). */
export const ROW_H = 26;
/** Lane pitch in px. */
export const LANE_W = 14;
/** Padding left of the lanes. */
export const PAD = 6;
/** Room right of the last lane, before the message column (matches --graph-pad-r): keeps the nodes clear of the text. */
export const PAD_R = 18;
/** Elbow radius. */
export const R = 7;
/** Stroke of the lane-colored ring around a node's avatar disc (AVATAR_R, util/identicon.ts). */
export const RING_W = 1.5;
/** Node radius: the ring's outer edge. */
export const NODE_R = AVATAR_R + RING_W;
/** Narrowest graph column. */
export const MIN_GRAPH_W = 28;
/** Narrowest and widest branch / tag column (left of the graph). */
export const MIN_REFS_W = 64;
export const MAX_REFS_W = 400;
/** Narrowest subject column: the columns right of it give way (see layoutColumns) before it does. */
export const MIN_SUBJECT_W = 200;
/** The author column once it shows only the avatar. */
export const AVATAR_COL_W = 28;
/** The time slot that replaces the date column: room for the time marks' widest text, by date format. */
export const TIME_SLOT_W = { relative: 52, absolute: 92 } as const;
/**
 * Most lanes the column shows. The backend drops edges whose both ends lie past lane 63, so lanes
 * from here on would show nodes without their lines. Keep equal to `MAX_SENT_LANE + 1` (graph_cache.rs).
 */
export const MAX_LANES = 64;

/** Palette size; color index = lane % COLORS. */
const COLORS = 8;
/** Vertical offset of the horizontals when a node has sideways lines both entering and leaving. */
const SPLIT = 2;
/** Batched path classes: solid by color, dashed, then the same for fading open edges. */
const CLASSES = [
  ...Array.from({ length: COLORS }, (_, c) => `c${c}`),
  'd',
  ...Array.from({ length: COLORS }, (_, c) => `c${c} o`),
  'd o',
];

/** Lane center x. Integer for every lane, so 2px strokes stay crisp. */
export function cx(lane: number): number {
  return PAD + lane * LANE_W + LANE_W / 2;
}

/** Width the lanes need: all of maxLanes visible, but never more than MAX_LANES of them. */
export function naturalWidth(maxLanes: number): number {
  return Math.max(MIN_GRAPH_W, Math.min(maxLanes, MAX_LANES) * LANE_W + PAD + PAD_R);
}

/**
 * Left edge of a row's highlight band in row px: its node's center (the branch / tag column and the
 * lanes left of the node stay on the graph background). A node past the column's right edge counts as
 * sitting on it.
 */
export function bandStart(lane: number, refsW: number, graphW: number): number {
  return refsW + Math.min(cx(lane), graphW);
}

/** Graph column width: the lanes' natural width, capped by the user's column width (also a saved one). */
export function graphWidth(maxLanes: number, cap: number): number {
  return Math.min(Math.max(cap, MIN_GRAPH_W), naturalWidth(maxLanes));
}

/** Branch / tag column width: the user's column width (also a saved one) within the limits. */
export function refsWidth(w: number): number {
  return Math.min(MAX_REFS_W, Math.max(MIN_REFS_W, w));
}

/**
 * Width of a row's branch / tag cell: from the row's left edge to 2px short of the row's node's ring
 * (the graph column starts at `refsW`; a node beyond the column counts as sitting on its right edge).
 */
export function refsCellWidth(lane: number, refsW: number, graphW: number): number {
  return refsW + Math.min(cx(lane), graphW) - NODE_R - 2;
}

export interface ColumnPrefs {
  /** Width of each column right of the subject while it is on; null = turned off. */
  author: number | null;
  date: number | null;
  sha: number | null;
  /** Width of the time slot that may stand in for the date column (TIME_SLOT_W by date format). */
  timeSlot: number;
}

export interface ColumnLayout {
  /** Subject column: what the others leave, never under MIN_SUBJECT_W. */
  subject: number;
  /** Author column width (AVATAR_COL_W when compact); 0 when off. */
  author: number;
  /** The author column shows only the avatar. */
  compactAuthor: boolean;
  /** Date column width; 0 when off or given way. */
  date: number;
  /** Time slot width (the dates as time marks) in place of the date column; 0 when off or no room. */
  time: number;
  /** Sha column width; 0 when off. */
  sha: number;
  /** Everything right of the subject: author + date + time + sha. */
  right: number;
  /** The rows are wider than `avail`: the list scrolls sideways under the sticky right columns. */
  overflow: boolean;
}

/**
 * The columns right of the graph in `avail` px (Infinity while unmeasured: nothing gives way). The
 * subject keeps MIN_SUBJECT_W; the room left decides the rest, step by step: the full columns; the date
 * column as a time slot; the author as the avatar; the avatar without any time. When even that does not
 * fit, the rows overflow and scroll sideways while the right columns stick to the list's right edge.
 */
export function layoutColumns(avail: number, prefs: ColumnPrefs): ColumnLayout {
  const sha = prefs.sha ?? 0;
  const name = prefs.author ?? 0;
  const avatar = prefs.author == null ? 0 : AVATAR_COL_W;
  const slot = prefs.date == null ? 0 : prefs.timeSlot;
  const room = avail - MIN_SUBJECT_W;
  let author = name;
  let date = prefs.date ?? 0;
  let time = 0;
  if (name + date + sha > room) {
    date = 0;
    if (name + slot + sha <= room) time = slot;
    else {
      author = avatar;
      if (avatar + slot + sha <= room) time = slot;
    }
  }
  const right = author + date + time + sha;
  const subject = Number.isFinite(avail) ? Math.max(MIN_SUBJECT_W, avail - right) : MIN_SUBJECT_W;
  return {
    subject,
    author,
    compactAuthor: author !== name,
    date,
    time,
    sha,
    right,
    overflow: subject + right > avail,
  };
}

export type NodeKind = 'commit' | 'wip' | 'stash';

export interface LaneNode {
  row: number;
  x: number;
  y: number;
  kind: NodeKind;
  color: number;
  /** The author, for the commit's avatar. */
  author: string;
  email: string;
}

export interface LanePath {
  /** CSS class list: `c0`…`c7` (solid), `d` (dashed), plus `o` for fading open edges. */
  cls: string;
  d: string;
}

export interface LaneModel {
  paths: LanePath[];
  nodes: LaneNode[];
  /** SVG height in px; the SVG's top sits one row above row `start`. */
  height: number;
}

/** Leaves its node sideways: merge links (and any node edge that does not end at the next node). */
function leavesNode(flags: number): boolean {
  return (flags & EDGE_MERGE) !== 0 || ((flags & EDGE_FROM_NODE) !== 0 && (flags & EDGE_TO_NODE) === 0);
}

/** Enters the next row's node sideways (forks converging into their parent). */
function entersNode(flags: number): boolean {
  return (flags & EDGE_TO_NODE) !== 0 && !leavesNode(flags);
}

function hasSideways(row: GraphRow | undefined, test: (flags: number) => boolean): boolean {
  if (!row) return false;
  for (const e of row.edges) if (e[0] !== e[1] && test(e[3])) return true;
  return false;
}

interface Run {
  bucket: number;
  y0: number;
  y1: number;
}

/**
 * Paths and nodes for rows [start, end) (end clamped to total by the caller). Edges of rows
 * start-1 … end-1 are drawn; y is relative to the SVG top, which sits at row start-1.
 * Unloaded rows (get → undefined) are skipped. `width` culls lanes outside the column, and nodes in
 * lanes ≥ MAX_LANES are never drawn (their lines are not sent). Edges with one end inside are drawn
 * and leave the column at its right edge.
 */
export function buildLanes(
  get: (i: number) => GraphRow | undefined,
  start: number,
  end: number,
  total: number,
  width: number,
): LaneModel {
  const parts: string[][] = CLASSES.map(() => []);
  const runs = new Map<number, Run>();
  // The widest column's right padding still holds the start of lane MAX_LANES: cap by MAX_LANES too.
  const lastLane = Math.min(MAX_LANES - 1, Math.floor((width - PAD) / LANE_W));
  const yc = (r: number) => (r - start + 1) * ROW_H + ROW_H / 2;
  const flush = (lane: number, run: Run) => {
    parts[run.bucket].push(`M${cx(lane)} ${run.y0}V${run.y1}`);
  };

  const first = Math.max(0, start - 1);
  let prev = first > 0 ? get(first - 1) : undefined;
  let row = first < total ? get(first) : undefined;
  for (let r = first; r < end; r++) {
    const next = r + 1 < total ? get(r + 1) : undefined;
    if (row) {
      const y1 = yc(r);
      const y2 = y1 + ROW_H;
      // Both incoming and outgoing horizontals at one node: in at cy−2, out at cy+2.
      const yOut = y1 + (hasSideways(prev, entersNode) && hasSideways(row, leavesNode) ? SPLIT : 0);
      const yIn = y2 - (hasSideways(row, entersNode) && hasSideways(next, leavesNode) ? SPLIT : 0);
      for (const [from, to, color, flags] of row.edges) {
        if (from > lastLane && to > lastLane) continue;
        const base = flags & EDGE_DASHED ? COLORS : color % COLORS;
        const x1 = cx(from);
        const x2 = cx(to);
        const s = x2 > x1 ? 1 : -1;
        if (flags & EDGE_OPEN) {
          // Towards a parent that is not loaded: stop at the bottom of the last row (faded by class).
          const yb = y1 + ROW_H / 2;
          parts[base + COLORS + 1].push(
            x1 === x2 ? `M${x1} ${y1}V${yb}` : `M${x1} ${yOut}H${x2 - s * R}Q${x2} ${yOut} ${x2} ${yOut + R}V${yb}`,
          );
        } else if (x1 === x2) {
          const run = runs.get(from);
          if (run && run.bucket === base && run.y1 === y1) run.y1 = y2;
          else {
            if (run) flush(from, run);
            runs.set(from, { bucket: base, y0: y1, y1: y2 });
          }
        } else if (leavesNode(flags)) {
          parts[base].push(`M${x1} ${yOut}H${x2 - s * R}Q${x2} ${yOut} ${x2} ${yOut + R}V${y2}`);
        } else if (flags & EDGE_TO_NODE) {
          parts[base].push(`M${x1} ${y1}V${yIn - R}Q${x1} ${yIn} ${x1 + s * R} ${yIn}H${x2}`);
        } else {
          const ym = y1 + ROW_H / 2;
          parts[base].push(`M${x1} ${y1}C${x1} ${ym} ${x2} ${ym} ${x2} ${y2}`);
        }
      }
    }
    prev = row;
    row = next;
  }
  for (const [lane, run] of runs) flush(lane, run);

  const nodes: LaneNode[] = [];
  for (let r = start; r < end; r++) {
    const n = get(r);
    if (!n || n.lane > lastLane) continue;
    const kind: NodeKind = n.kind === 'wip' ? 'wip' : n.kind === 'stash' ? 'stash' : 'commit';
    nodes.push({ row: r, x: cx(n.lane), y: yc(r), kind, color: n.color % COLORS, author: n.author, email: n.email });
  }

  const paths: LanePath[] = [];
  parts.forEach((p, b) => {
    if (p.length) paths.push({ cls: CLASSES[b], d: p.join('') });
  });
  // Down to the bottom of row `end` when it exists; never past the list (no extra scroll height).
  const height = (end - start + 1 + (end < total ? 1 : 0)) * ROW_H;
  return { paths, nodes, height };
}
