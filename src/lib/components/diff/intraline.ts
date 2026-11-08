// Intra-line (word) change ranges between a paired deleted/added line (diff@9 diffWordsWithSpace).
import { diffWordsWithSpace } from 'diff';
import type { DiffModel } from './model';

/** [start, end) UTF-16 offsets into the line text. */
export type Range = [number, number];

export interface ChangeRanges {
  old: Range[];
  new: Range[];
}

const MAX_LINE = 1000;
const MAX_EDIT = 100;
const MAX_RANGES = 12;
const MIN_SIMILARITY = 0.5;
const BLANK = /^\s*$/;

function add(out: Range[], start: number, end: number, text: string): void {
  const last = out[out.length - 1];
  // Merge with the previous range across a gap of ≤ 1 char or whitespace only (less confetti).
  if (last && (start - last[1] <= 1 || BLANK.test(text.slice(last[1], start)))) last[1] = end;
  else out.push([start, end]);
}

/**
 * Changed character ranges of each side, or null when the pair is not worth marking: an empty,
 * identical or very long line, too many edits, too many ranges, or lines too dissimilar.
 */
export function changeRanges(a: string, b: string): ChangeRanges | null {
  if (a === b || !a || !b || a.length > MAX_LINE || b.length > MAX_LINE) return null;
  const parts = diffWordsWithSpace(a, b, { maxEditLength: MAX_EDIT });
  if (!parts) return null;
  const old: Range[] = [];
  const neu: Range[] = [];
  let io = 0;
  let ino = 0;
  let same = 0;
  for (const p of parts) {
    const len = p.value.length;
    if (p.removed) {
      add(old, io, io + len, a);
      io += len;
    } else if (p.added) {
      add(neu, ino, ino + len, b);
      ino += len;
    } else {
      io += len;
      ino += len;
      same += len;
    }
  }
  if (2 * same < MIN_SIMILARITY * (a.length + b.length)) return null;
  if (old.length > MAX_RANGES || neu.length > MAX_RANGES) return null;
  return { old, new: neu };
}

/**
 * Lazy, memoized intra-line lookup for a model: ranges of line `i` (a paired deleted or added line),
 * else null. Create one per model (the cache is keyed by deleted-line index).
 */
export function intraLookup(model: DiffModel): (i: number) => Range[] | null {
  const cache = new Map<number, ChangeRanges | null>();
  let back: Map<number, number> | null = null;
  return (i) => {
    const ln = model.lines[i];
    if (!ln || ln.kind === 0) return null;
    let del: number | undefined;
    let added: number | undefined;
    if (ln.kind === 2) {
      del = i;
      added = model.pairs.get(i);
    } else {
      if (!back) {
        back = new Map();
        for (const [d, a] of model.pairs) back.set(a, d);
      }
      del = back.get(i);
      added = i;
    }
    if (del === undefined || added === undefined) return null;
    let r = cache.get(del);
    if (r === undefined) {
      r = changeRanges(model.lines[del].text, model.lines[added].text);
      cache.set(del, r);
    }
    if (!r) return null;
    return ln.kind === 2 ? r.old : r.new;
  };
}
