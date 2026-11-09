// Author identicons (GitKraken-style graph avatars, without the network): a 7×7 pixel glyph mirrored
// left to right, in a hue from the lane palette, both from a hash of the author's email (the name when
// there is none). GraphLanes draws one at every commit node; Avatar in the details panel and the
// compact author column. Glyph paths are cached per author.

/** Radius of the avatar disc in SVG units: 7 cells of CELL inside a half-cell margin (16 across). */
export const AVATAR_R = 8;
/** Cells per side of the glyph. */
const CELLS = 7;
/** Cell size in SVG units. */
const CELL = 2;
/** Columns the hash decides: the three left ones and the middle; the right ones mirror the left. */
const HALF = (CELLS + 1) / 2;
/** Palette size: `--lane-0` … `--lane-7`. */
const HUES = 8;
const CACHE_MAX = 4096;

export interface Identicon {
  /** Palette index: the author's hue is `--lane-{hue}`. */
  hue: number;
  /** Glyph path: squares centered on the origin, within ±(CELLS × CELL / 2) = ±7; fill it in the hue. */
  path: string;
}

/** Author identity: the normalized email, the name when there is none (one avatar per person, in every repository). */
export function avatarKey(name: string, email: string): string {
  return email.trim().toLowerCase() || name.trim().toLowerCase();
}

/** FNV-1a, 32-bit. */
function fnv1a(s: string): number {
  let h = 0x811c9dc5;
  for (let i = 0; i < s.length; i++) {
    h ^= s.charCodeAt(i);
    h = Math.imul(h, 0x01000193);
  }
  return h >>> 0;
}

/** MurmurHash3 finalizer: spreads the hash so neighbouring cells do not correlate. */
function mix(h: number): number {
  h ^= h >>> 16;
  h = Math.imul(h, 0x85ebca6b);
  h ^= h >>> 13;
  h = Math.imul(h, 0xc2b2ae35);
  h ^= h >>> 16;
  return h >>> 0;
}

/**
 * The glyph for 28 hash bits: bit `row × HALF + column` turns cell (row, column) and its mirror on.
 * Each run of cells in a row is one rectangle, so adjacent cells share no antialiased seam.
 */
function glyphPath(bits: number): string {
  const o = -(CELLS * CELL) / 2;
  const parts: string[] = [];
  for (let r = 0; r < CELLS; r++) {
    const y = o + r * CELL;
    let run = -1;
    for (let c = 0; c <= CELLS; c++) {
      const on = c < CELLS && ((bits >>> (r * HALF + Math.min(c, CELLS - 1 - c))) & 1) === 1;
      if (on && run < 0) run = c;
      else if (!on && run >= 0) {
        const w = (c - run) * CELL;
        parts.push(`M${o + run * CELL} ${y}h${w}v${CELL}h${-w}z`);
        run = -1;
      }
    }
  }
  return parts.join('');
}

const cache = new Map<string, Identicon>();

/** The author's identicon (cached). */
export function identicon(name: string, email: string): Identicon {
  const key = avatarKey(name, email);
  let id = cache.get(key);
  if (!id) {
    const h = fnv1a(key);
    id = { hue: h % HUES, path: glyphPath(mix(h)) };
    if (cache.size >= CACHE_MAX) cache.clear();
    cache.set(key, id);
  }
  return id;
}
