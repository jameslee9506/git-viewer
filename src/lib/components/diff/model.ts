// Pure row models for the unified and split diff views (plus small diff helpers). No DOM, no Svelte.
import type { BlobSpec, ChangeStatus, DiffTarget, FileDiff } from '../../types';

/** Line flag: the line lacks a trailing newline (render ∅ after it). */
export const NOEOL = 1;
/** Line flag: the patch line ended with `\r` (stripped from `text`). */
export const CR = 2;
/** Line flag: the line's CR differs from its paired line on the other side (render a dim ␍). */
export const CRMARK = 4;

/** One line of the patch. `oi`/`ni` index into `oldLines`/`newLines` (-1 if absent). */
export interface DLine {
  kind: 0 | 1 | 2;
  old: number | null;
  new: number | null;
  text: string;
  flags: number;
  oi: number;
  ni: number;
}

export type HunkRow = { t: 'hunk'; heading: string; hidden: number };
export type URow = HunkRow | { t: 'line'; line: number };
/** left/right are line indexes, -1 = filler. */
export type SRow = HunkRow | { t: 'pair'; left: number; right: number };

export interface DiffModel {
  /** Every line of every hunk, patch order. */
  lines: DLine[];
  unified: URow[];
  split: SRow[];
  /** Side documents for highlighting (ctx+del / ctx+add). */
  oldLines: string[];
  newLines: string[];
  /** Side line indexes (> 0, strictly increasing) where a new hunk starts: the grammar restarts there. */
  oldResets: number[];
  newResets: number[];
  /** Deleted line index → paired added line index (intra-line pairs). */
  pairs: Map<number, number>;
  /** First row index of each change block (n/p navigation). */
  uniBlocks: number[];
  splitBlocks: number[];
  /** Longest line in columns (tab stops of 4, wide characters count 2). */
  maxCols: number;
  /** Largest line number on either side (gutter width). */
  maxNo: number;
  /** Line index → its row in `unified` / `split`. */
  uniRowOf: Int32Array;
  splitRowOf: Int32Array;
}

/** The function-context heading after the closing `@@` (`@@@` in combined diffs) of a hunk header. */
export function hunkHeading(header: string): string {
  let n = 0;
  while (header.charCodeAt(n) === 64) n++;
  if (n < 2) return '';
  // Ranges never contain '@': the first " @@" (as many '@' as the opening run) closes them.
  const close = header.indexOf(` ${'@'.repeat(n)}`, n);
  if (close < 0) return '';
  let s = header.slice(close + 1 + n);
  if (s.startsWith(' ')) s = s.slice(1);
  return s.trimEnd();
}

const NON_ASCII_OR_TAB = /[^\x20-\x7e]/;

function isWide(c: number): boolean {
  return (
    (c >= 0x1100 && c <= 0x115f) ||
    (c >= 0x2e80 && c <= 0xa4cf && c !== 0x303f) ||
    (c >= 0xac00 && c <= 0xd7a3) ||
    (c >= 0xf900 && c <= 0xfaff) ||
    (c >= 0xfe30 && c <= 0xfe4f) ||
    (c >= 0xff00 && c <= 0xff60) ||
    (c >= 0xffe0 && c <= 0xffe6)
  );
}

/** Display width of a line in monospace columns (tab stops of 4; CJK/Hangul/fullwidth = 2). */
export function columns(text: string): number {
  if (!NON_ASCII_OR_TAB.test(text)) return text.length;
  let col = 0;
  for (let i = 0; i < text.length; i++) {
    const c = text.charCodeAt(i);
    if (c === 9) col += 4 - (col % 4);
    else col += isWide(c) ? 2 : 1;
  }
  return col;
}

function pushReset(resets: number[], at: number): void {
  if (at > 0 && resets[resets.length - 1] !== at) resets.push(at);
}

/** Build both row models from a structured file diff. */
export function buildModel(diff: FileDiff): DiffModel {
  const lines: DLine[] = [];
  const unified: URow[] = [];
  const split: SRow[] = [];
  const oldLines: string[] = [];
  const newLines: string[] = [];
  const oldResets: number[] = [];
  const newResets: number[] = [];
  const pairs = new Map<number, number>();
  const uniBlocks: number[] = [];
  const splitBlocks: number[] = [];
  const uniRow: number[] = [];
  const splitRow: number[] = [];
  let maxCols = 0;
  let maxNo = 0;
  // First line number after the previous hunk, per side.
  let nextOld = 1;
  let nextNew = 1;
  // Current change block (maximal run of non-context lines).
  let dels: number[] = [];
  let adds: number[] = [];
  let inBlock = false;

  const push = (kind: 0 | 1 | 2, oldNo: number | null, newNo: number | null, raw: string): number => {
    let text = raw;
    let flags = 0;
    if (text.charCodeAt(text.length - 1) === 13) {
      text = text.slice(0, -1);
      flags = CR;
    }
    const oi = kind === 1 ? -1 : oldLines.push(text) - 1;
    const ni = kind === 2 ? -1 : newLines.push(text) - 1;
    if (text.length > maxCols) {
      const cols = columns(text);
      if (cols > maxCols) maxCols = cols;
    } else if (text.length * 4 > maxCols && NON_ASCII_OR_TAB.test(text)) {
      const cols = columns(text);
      if (cols > maxCols) maxCols = cols;
    }
    if (oldNo != null && oldNo > maxNo) maxNo = oldNo;
    if (newNo != null && newNo > maxNo) maxNo = newNo;
    uniRow.push(-1);
    splitRow.push(-1);
    return lines.push({ kind, old: oldNo, new: newNo, text, flags, oi, ni }) - 1;
  };

  const flush = () => {
    if (!inBlock) return;
    splitBlocks.push(split.length);
    const n = Math.max(dels.length, adds.length);
    for (let k = 0; k < n; k++) {
      const left = k < dels.length ? dels[k] : -1;
      const right = k < adds.length ? adds[k] : -1;
      if (left >= 0) splitRow[left] = split.length;
      if (right >= 0) splitRow[right] = split.length;
      split.push({ t: 'pair', left, right });
    }
    const m = Math.min(dels.length, adds.length);
    for (let k = 0; k < m; k++) {
      const d = dels[k];
      const a = adds[k];
      pairs.set(d, a);
      const dl = lines[d];
      const al = lines[a];
      if ((dl.flags & CR) !== (al.flags & CR)) {
        if (dl.flags & CR) dl.flags |= CRMARK;
        else al.flags |= CRMARK;
      }
    }
    dels = [];
    adds = [];
    inBlock = false;
  };

  for (let g = 0; g < diff.hunks.length; g++) {
    const h = diff.hunks[g];
    const begOld = h.oldLines ? h.oldStart : h.oldStart + 1;
    const begNew = h.newLines ? h.newStart : h.newStart + 1;
    const hidden = Math.max(0, begOld - nextOld, begNew - nextNew);
    if (g > 0 || hidden > 0) {
      const row: HunkRow = { t: 'hunk', heading: hunkHeading(h.header), hidden };
      unified.push(row);
      split.push(row);
    }
    pushReset(oldResets, oldLines.length);
    pushReset(newResets, newLines.length);

    let last = -1;
    for (const [kind, oldNo, newNo, text] of h.lines) {
      if (kind === 3) {
        if (last >= 0) lines[last].flags |= NOEOL;
        continue;
      }
      if (kind === 0) {
        flush();
        const i = push(0, oldNo, newNo, text);
        uniRow[i] = unified.length;
        splitRow[i] = split.length;
        unified.push({ t: 'line', line: i });
        split.push({ t: 'pair', left: i, right: i });
        last = i;
        continue;
      }
      if (!inBlock) {
        inBlock = true;
        uniBlocks.push(unified.length);
      }
      const i = push(kind, kind === 2 ? oldNo : null, kind === 1 ? newNo : null, text);
      uniRow[i] = unified.length;
      unified.push({ t: 'line', line: i });
      if (kind === 2) dels.push(i);
      else adds.push(i);
      last = i;
    }
    flush();
    nextOld = h.oldLines ? h.oldStart + h.oldLines : h.oldStart + 1;
    nextNew = h.newLines ? h.newStart + h.newLines : h.newStart + 1;
  }

  while (oldResets.length && oldResets[oldResets.length - 1] >= oldLines.length) oldResets.pop();
  while (newResets.length && newResets[newResets.length - 1] >= newLines.length) newResets.pop();

  return {
    lines,
    unified,
    split,
    oldLines,
    newLines,
    oldResets,
    newResets,
    pairs,
    uniBlocks,
    splitBlocks,
    maxCols,
    maxNo,
    uniRowOf: Int32Array.from(uniRow),
    splitRowOf: Int32Array.from(splitRow),
  };
}

// ───────────── navigation ─────────────

/**
 * Index into `blocks` (sorted row indexes) of the next block starting after `anchor` (dir 1) or the
 * previous block starting before it (dir -1); -1 when there is none.
 */
export function findBlock(blocks: readonly number[], anchor: number, dir: 1 | -1): number {
  let lo = 0;
  let hi = blocks.length;
  if (dir === 1) {
    while (lo < hi) {
      const mid = (lo + hi) >> 1;
      if (blocks[mid] <= anchor) lo = mid + 1;
      else hi = mid;
    }
    return lo < blocks.length ? lo : -1;
  }
  while (lo < hi) {
    const mid = (lo + hi) >> 1;
    if (blocks[mid] < anchor) lo = mid + 1;
    else hi = mid;
  }
  return lo - 1;
}

/** Hunk navigation keys: n / ⌥↓ → 1, p / ⌥↑ → -1, anything else → 0. Matches on `code` (IME-safe). */
export function blockNavKey(e: KeyboardEvent): 1 | -1 | 0 {
  if (e.isComposing || e.keyCode === 229 || e.metaKey || e.ctrlKey || e.shiftKey) return 0;
  if (e.altKey) return e.code === 'ArrowDown' ? 1 : e.code === 'ArrowUp' ? -1 : 0;
  return e.code === 'KeyN' ? 1 : e.code === 'KeyP' ? -1 : 0;
}

/** True for rows that belong to a change block (not context, not a hunk header). */
export function isChangeRow(model: DiffModel, view: 'unified' | 'split', row: number): boolean {
  if (view === 'unified') {
    const r = model.unified[row];
    return !!r && r.t === 'line' && model.lines[r.line].kind !== 0;
  }
  const r = model.split[row];
  return !!r && r.t === 'pair' && r.left !== r.right;
}

// ───────────── scroll memory ─────────────

/**
 * Where a diff was scrolled to: line numbers of the first line at the top of the viewport (hunk = its
 * header row was on top). Line numbers survive view, context and whitespace changes.
 */
export interface TopPos {
  old: number | null;
  new: number | null;
  hunk: boolean;
}

const MEMORY_MAX = 400;
const memory = new Map<string, TopPos>();

/** Remember the top position of a diff item (shared by the unified and split views). */
export function rememberTop(key: string, pos: TopPos): void {
  memory.delete(key);
  memory.set(key, pos);
  if (memory.size > MEMORY_MAX) memory.delete(memory.keys().next().value as string);
}

export function recallTop(key: string): TopPos | undefined {
  return memory.get(key);
}

/** Top position for a row of a view (the first line at or after it). */
export function topPosOf(model: DiffModel, view: 'unified' | 'split', row: number): TopPos | null {
  const rows: (URow | SRow)[] = view === 'unified' ? model.unified : model.split;
  for (let i = row; i < rows.length; i++) {
    const r = rows[i];
    if (r.t === 'hunk') continue;
    // A pair records its left line when present: unified lists the deleted line before the added one.
    const ln = model.lines[r.t === 'line' ? r.line : r.left >= 0 ? r.left : r.right];
    return { old: ln.old, new: ln.new, hunk: i !== row };
  }
  return null;
}

/** Row of a view to scroll to for a remembered position: the first line at or after its line number. */
export function rowOfTopPos(model: DiffModel, view: 'unified' | 'split', pos: TopPos): number {
  const lines = model.lines;
  const byNew = pos.new != null;
  const want = (byNew ? pos.new : pos.old) ?? 0;
  let line = -1;
  for (let i = 0; i < lines.length; i++) {
    const n = byNew ? lines[i].new : lines[i].old;
    if (n != null && n >= want) {
      line = i;
      break;
    }
  }
  const rows: (URow | SRow)[] = view === 'unified' ? model.unified : model.split;
  if (line < 0) return Math.max(0, rows.length - 1);
  const exact = (byNew ? lines[line].new : lines[line].old) === want;
  const row = (view === 'unified' ? model.uniRowOf : model.splitRowOf)[line];
  return pos.hunk && exact && row > 0 && rows[row - 1].t === 'hunk' ? row - 1 : row;
}

// ───────────── file-level helpers ─────────────

/** Old/new shas of a submodule (gitlink) diff, or null when the diff is not one. */
export function submoduleShas(diff: FileDiff): { old: string | null; new: string | null } | null {
  if (diff.binary || diff.hunks.length !== 1) return null;
  const re = /^Subproject commit ([0-9a-f]{7,64}(?:-dirty)?)\s*$/;
  let oldSha: string | null = null;
  let newSha: string | null = null;
  let n = 0;
  for (const [kind, , , text] of diff.hunks[0].lines) {
    if (kind === 3) continue;
    const m = re.exec(text);
    if (!m || ++n > 2) return null;
    if (kind === 2) oldSha = m[1];
    else if (kind === 1) newSha = m[1];
    else oldSha = newSha = m[1];
  }
  return n > 0 ? { old: oldSha, new: newSha } : null;
}

/** Equal content (used to skip re-rendering when a working-tree refetch returns the same diff). */
export function sameDiff(a: FileDiff, b: FileDiff): boolean {
  if (a === b) return true;
  if (
    a.path !== b.path ||
    a.oldPath !== b.oldPath ||
    a.binary !== b.binary ||
    a.tooLarge !== b.tooLarge ||
    a.oldOid !== b.oldOid ||
    a.newOid !== b.newOid ||
    a.oldMode !== b.oldMode ||
    a.newMode !== b.newMode ||
    a.additions !== b.additions ||
    a.deletions !== b.deletions ||
    a.headerLines.length !== b.headerLines.length ||
    a.hunks.length !== b.hunks.length
  )
    return false;
  for (let i = 0; i < a.headerLines.length; i++) if (a.headerLines[i] !== b.headerLines[i]) return false;
  for (let i = 0; i < a.hunks.length; i++) {
    const x = a.hunks[i];
    const y = b.hunks[i];
    if (x.header !== y.header || x.lines.length !== y.lines.length) return false;
    for (let j = 0; j < x.lines.length; j++) {
      const p = x.lines[j];
      const q = y.lines[j];
      if (p[0] !== q[0] || p[1] !== q[1] || p[2] !== q[2] || p[3] !== q[3]) return false;
    }
  }
  return true;
}

export interface SideSpecs {
  old: BlobSpec | null;
  new: BlobSpec | null;
}

/**
 * Blob specs of the two sides of a file diff (images, sizes). `repo` is the repo id; `oldOid` is the
 * diff's old blob id (`FileDiff.oldOid`), which names the old side of staged and conflicted diffs:
 * `HEAD` of another worktree cannot be named from the repo, and a conflict's old side is the "ours"
 * blob `diff-files -2` compared (no blob when that printed no index line).
 */
export function sideSpecs(
  target: DiffTarget,
  path: string,
  oldPath: string | null,
  status: ChangeStatus,
  repo: string,
  oldOid: string | null = null,
): SideSpecs {
  const from = oldPath ?? path;
  const oldBlob: BlobSpec | null = oldOid ? { kind: 'oid', oid: oldOid } : null;
  let old: BlobSpec | null = null;
  let neu: BlobSpec | null = null;
  switch (target.kind) {
    case 'commit':
      old = { kind: 'rev', rev: target.parent ?? `${target.oid}^`, path: from };
      neu = { kind: 'rev', rev: target.oid, path };
      break;
    case 'range':
      old = { kind: 'rev', rev: target.from, path: from };
      neu = { kind: 'rev', rev: target.to, path };
      break;
    case 'worktree':
      old = { kind: 'rev', rev: target.oid, path: from };
      neu = { kind: 'worktree', cwd: target.cwd, path };
      break;
    case 'staged':
      old = oldBlob ?? (target.cwd === repo ? { kind: 'rev', rev: 'HEAD', path: from } : null);
      neu = { kind: 'index', cwd: target.cwd, path };
      break;
    case 'unstaged':
      // Intent-to-add renames (`2 .R`) carry oldPath: the index side is the source file.
      old = { kind: 'index', cwd: target.cwd, path: from };
      neu = { kind: 'worktree', cwd: target.cwd, path };
      break;
    case 'conflicted':
      // `diff-files -2`: the old side is "ours" (stage 2).
      old = oldBlob;
      neu = { kind: 'worktree', cwd: target.cwd, path };
      break;
    case 'untracked':
      neu = { kind: 'worktree', cwd: target.cwd, path };
      break;
    case 'stash':
      old = { kind: 'rev', rev: `${target.oid}^1`, path: from };
      neu = { kind: 'rev', rev: target.oid, path };
      break;
    case 'stashUntracked':
      neu = { kind: 'rev', rev: `${target.oid}^3`, path };
      break;
  }
  if (status === 'A' || status === '?') old = null;
  if (status === 'D') neu = null;
  return { old, new: neu };
}
