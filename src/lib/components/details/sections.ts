// Heights of the working-tree panel's file sections (WipDetails: Conflicted, Unstaged, Staged), which
// share the room between the panel's bar and the commit box. A section is as tall as its files — the
// flex layout shares the room out — until its bottom edge is dragged: from then on it has the height
// it was given, whatever it holds, until a double-click on the edge lets it follow its files again.
// A set height is a wish: what shows is what the room allows (`sectionHeights`), and the wish stays.

/** A section, as these rules see it. */
export interface SectionBox {
  /**
   * The least it takes: its header alone (closed, or open with neither files nor a set height), else
   * its header, up to three rows and its edge.
   */
  min: number;
  /** The height its edge was dragged to in whole px, 0: none (it follows its files, or is closed). */
  set: number;
}

/**
 * The height of every section that has one set (0 for the others: the layout sizes them) in `avail`
 * px of room (0: not measured yet, nothing is cut). Every section keeps its minimum. What the room
 * holds beyond those goes to the set heights from the top down — a section too many squeezes the
 * ones below it, the nearest to what takes the room (the commit box, as its message grows), and
 * what is above stays put — and the sections that follow their files share the rest.
 */
export function sectionHeights(avail: number, boxes: readonly SectionBox[]): number[] {
  let spare = Number.POSITIVE_INFINITY;
  if (avail > 0) {
    spare = avail;
    for (const b of boxes) spare -= b.min;
    spare = Math.max(0, spare);
  }
  return boxes.map((b) => {
    if (!(b.set > 0)) return 0;
    const extra = Math.min(Math.max(0, b.set - b.min), spare);
    spare -= extra;
    return b.min + extra;
  });
}

/**
 * The tallest a drag of its edge makes section `i`: the sections above it stay as tall as they are
 * (`shown`: every section's height on screen) and the ones below keep their minimum. The edge under
 * the pointer never moves what is above it, and stops where the room ends.
 */
export function sectionMax(
  avail: number,
  boxes: readonly SectionBox[],
  shown: readonly number[],
  i: number,
): number {
  let room = avail;
  for (let j = 0; j < boxes.length; j++) {
    if (j < i) room -= shown[j];
    else if (j > i) room -= boxes[j].min;
  }
  return Math.max(boxes[i].min, room);
}

/**
 * The sections above `i` that a drag of its edge to `px` would resize: they follow their files, and
 * either are cut short already (`cut(j)`: the list scrolls — it would grow into the room a smaller
 * `i` leaves) or would be (the sections no longer fit the room, and the layout takes from every one
 * that follows its files, wherever it is). The caller gives them the height they have, so that the
 * edge stays under the pointer: a drag settles what is above it.
 */
export function heldAbove(
  avail: number,
  boxes: readonly SectionBox[],
  shown: readonly number[],
  cut: (j: number) => boolean,
  i: number,
  px: number,
): number[] {
  let total = px;
  for (let j = 0; j < boxes.length; j++) if (j !== i) total += shown[j];
  const over = avail > 0 && total > avail;
  const held: number[] = [];
  for (let j = 0; j < i; j++) {
    if (boxes[j].set > 0) continue;
    if (cut(j) || (over && shown[j] > boxes[j].min)) held.push(j);
  }
  return held;
}
