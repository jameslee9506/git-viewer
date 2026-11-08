//! Pure lane layout for the commit graph. No git access here; fully unit-testable.
//!
//! Input rows are in display order (children before parents). For each row we compute the node lane,
//! its color, and the edges from this row's center to the next row's center.
//!
//! Lanes are stable (never compacted; freed lanes are reused), so a line only moves sideways where it
//! leaves or enters a node. The first parent always continues in the node's own lane; a 2nd+ parent
//! joins the nearest line already heading to it, else opens the lowest free lane right of the node.
//! A 2nd+ parent on the very next row joins the lowest line heading to it (the lane its node takes
//! and the link is drawn down), so the link never paints another line's color over that lane.

use crate::model::{Edge, EDGE_DASHED, EDGE_FROM_NODE, EDGE_MERGE, EDGE_OPEN, EDGE_TO_NODE};

/// Parent reference: index of the parent row (> own index), or any value >= rows.len() for a parent
/// that is not loaded (truncated history). Equal out-of-range values denote the same missing commit.
pub type ParentRef = u32;

#[derive(Clone, Debug, Default)]
pub struct NodeIn {
    pub parents: Vec<ParentRef>,
    /// WIP / stash rows: links to parents (and the lines they start) are dashed.
    pub dashed: bool,
}

#[derive(Clone, Debug, Default)]
pub struct Layout {
    /// Node lane per row.
    pub lanes: Vec<u16>,
    /// Node color (palette index) per row.
    pub colors: Vec<u8>,
    /// All edges, row by row: row i owns `edges[offsets[i]..offsets[i + 1]]` (edges from row i to row i+1).
    pub edges: Vec<Edge>,
    /// `rows + 1` entries.
    pub offsets: Vec<u32>,
    /// Widest lane count over all rows (node lanes and edge endpoints).
    pub max_lanes: u32,
}

impl Layout {
    /// Edges leaving row `i` towards row `i + 1`.
    pub fn row_edges(&self, i: usize) -> &[Edge] {
        let a = self.offsets[i] as usize;
        let b = self.offsets[i + 1] as usize;
        &self.edges[a..b]
    }
}

/// Number of distinct lane colors; color = lane % PALETTE.
pub const PALETTE: u16 = 8;

/// A line occupying a lane at the current row.
#[derive(Clone, Copy, Debug)]
struct Slot {
    /// Row (or missing-parent id) the line is heading to.
    expect: u32,
    color: u8,
    /// Started by a dashed link and not joined by a solid one since.
    dashed: bool,
    /// False for the phantom pin reservation until a merge link joins it.
    visible: bool,
}

/// A line leaving row i-1 that lands in `slot` at row i, or ends at row i's node when that slot is hit.
#[derive(Clone, Copy, Debug)]
struct Out {
    from: u32,
    slot: u32,
    color: u8,
    flags: u8,
}

/// Compute the layout. `pin`: row that must sit in lane 0 (the WIP row, else HEAD's row); lane 0 is
/// reserved for it from the top, so its first-parent chain stays in lane 0 (None = no pinning; a row
/// outside `nodes` is ignored). Deterministic, O(rows × active lanes).
pub fn layout(nodes: &[NodeIn], pin: Option<u32>) -> Layout {
    let n = nodes.len();
    let mut lanes = Vec::with_capacity(n);
    let mut colors = Vec::with_capacity(n);
    let mut edges = Vec::with_capacity(n.saturating_mul(2));
    let mut offsets = Vec::with_capacity(n + 1);
    let mut widest = 0u32;

    let mut slots: Vec<Option<Slot>> = Vec::new();
    let mut outs: Vec<Out> = Vec::new();
    let mut hits: Vec<u32> = Vec::new();
    let mut passing: Vec<u32> = Vec::new();
    if let Some(p) = pin.filter(|&p| (p as usize) < n) {
        slots.push(Some(Slot { expect: p, color: 0, dashed: false, visible: false }));
    }

    for (i, node) in nodes.iter().enumerate() {
        let row = i as u32;
        // Lines ending here (ascending), visible lines passing by, first free lane: all before any change.
        hits.clear();
        passing.clear();
        let mut free = None;
        for (s, slot) in slots.iter().enumerate() {
            match slot {
                Some(sl) if sl.expect == row => hits.push(s as u32),
                Some(sl) => {
                    if sl.visible {
                        passing.push(s as u32);
                    }
                }
                None => {
                    if free.is_none() {
                        free = Some(s);
                    }
                }
            }
        }
        let lane = match (hits.first(), free) {
            (Some(&h), _) => h as usize,
            (None, Some(f)) => f,
            (None, None) => {
                slots.push(None);
                slots.len() - 1
            }
        };
        let color = lane_color(lane);

        // Every pending line targets a live slot; it ends at this node exactly when that slot is a hit.
        for o in outs.drain(..) {
            let ends = matches!(slots[o.slot as usize], Some(sl) if sl.expect == row);
            let (to, flags) = if ends { (lane as u32, o.flags | EDGE_TO_NODE) } else { (o.slot, o.flags) };
            widest = widest.max(o.from.max(o.slot) + 1);
            edges.push(Edge(lane16(o.from), lane16(to), o.color, flags));
        }
        offsets.push(edges.len() as u32);

        for &h in &hits {
            slots[h as usize] = None;
        }
        let kind = if node.dashed { EDGE_DASHED } else { 0 };
        let parents = node.parents.as_slice();
        if let Some(&p0) = parents.first() {
            slots[lane] = Some(Slot { expect: p0, color, dashed: node.dashed, visible: true });
            outs.push(Out { from: lane as u32, slot: lane as u32, color, flags: EDGE_FROM_NODE | kind });
        }
        for (j, &p) in parents.iter().enumerate().skip(1) {
            if parents[..j].contains(&p) {
                continue;
            }
            // A parent on the next row is the exception to "nearest": every line heading there ends at
            // its node, which takes the lowest of them, and the link is drawn down that lane, so it
            // joins that line (its color; a solid link clears its dashes), not one it never touches.
            // (`p < n`: on the last row, `row + 1` is the first missing-parent id, not a row.)
            let target = if p.checked_sub(row) == Some(1) && (p as usize) < n {
                slots.iter().position(|s| matches!(s, Some(sl) if sl.expect == p))
            } else {
                nearest(&slots, lane, p)
            };
            let joined = target.and_then(|s| {
                slots[s].as_mut().map(|sl| {
                    if !sl.visible {
                        sl.visible = true;
                    } else if !node.dashed {
                        sl.dashed = false;
                    }
                    (s, sl.color)
                })
            });
            let (s, c) = match joined {
                Some(found) => found,
                None => {
                    let s = match slots[lane + 1..].iter().position(Option::is_none) {
                        Some(k) => lane + 1 + k,
                        None => {
                            slots.push(None);
                            slots.len() - 1
                        }
                    };
                    let c = lane_color(s);
                    slots[s] = Some(Slot { expect: p, color: c, dashed: node.dashed, visible: true });
                    (s, c)
                }
            };
            let flags = EDGE_FROM_NODE | EDGE_MERGE | kind;
            outs.push(Out { from: lane as u32, slot: s as u32, color: c, flags });
        }
        for &s in &passing {
            if let Some(sl) = slots[s as usize] {
                let flags = if sl.dashed { EDGE_DASHED } else { 0 };
                outs.push(Out { from: s, slot: s, color: sl.color, flags });
            }
        }
        while matches!(slots.last(), Some(None)) {
            slots.pop();
        }

        lanes.push(lane16(lane as u32));
        colors.push(color);
        widest = widest.max(lane as u32 + 1);
    }

    for o in outs.drain(..) {
        widest = widest.max(o.from.max(o.slot) + 1);
        edges.push(Edge(lane16(o.from), lane16(o.slot), o.color, o.flags | EDGE_OPEN));
    }
    offsets.push(edges.len() as u32);
    edges.shrink_to_fit();
    Layout { lanes, colors, edges, offsets, max_lanes: widest }
}

/// Lane nearest to `lane` whose line heads to `p` (ties: the lower lane), including the phantom pin slot.
fn nearest(slots: &[Option<Slot>], lane: usize, p: u32) -> Option<usize> {
    let mut below = None;
    for (s, slot) in slots.iter().enumerate() {
        if matches!(slot, Some(sl) if sl.expect == p) {
            if s < lane {
                below = Some(s);
            } else {
                return match below {
                    Some(b) if lane - b <= s - lane => Some(b),
                    _ => Some(s),
                };
            }
        }
    }
    below
}

fn lane_color(lane: usize) -> u8 {
    (lane % PALETTE as usize) as u8
}

/// Lanes past `u16::MAX` (never drawn) saturate instead of wrapping onto visible lanes.
fn lane16(lane: u32) -> u16 {
    u16::try_from(lane).unwrap_or(u16::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    const D: u8 = EDGE_DASHED;
    const F: u8 = EDGE_FROM_NODE;
    const T: u8 = EDGE_TO_NODE;
    const M: u8 = EDGE_MERGE;
    const O: u8 = EDGE_OPEN;

    fn c(parents: &[u32]) -> NodeIn {
        NodeIn { parents: parents.to_vec(), dashed: false }
    }

    fn d(parents: &[u32]) -> NodeIn {
        NodeIn { parents: parents.to_vec(), dashed: true }
    }

    fn e(from: u16, to: u16, color: u8, flags: u8) -> Edge {
        Edge(from, to, color, flags)
    }

    /// Lays out `nodes`, checks every invariant, then compares with the exact expected result.
    fn expect(nodes: &[NodeIn], pin: Option<u32>, lanes: &[u16], colors: &[u8], edges: &[&[Edge]], max_lanes: u32) {
        let l = run(nodes, pin);
        assert_eq!(l.lanes, lanes, "lanes");
        assert_eq!(l.colors, colors, "colors");
        assert_eq!(edges.len(), nodes.len(), "test lists every row");
        for (i, want) in edges.iter().enumerate() {
            assert_eq!(l.row_edges(i), *want, "edges of row {i}");
        }
        assert_eq!(l.max_lanes, max_lanes, "max_lanes");
    }

    /// Lays out `nodes` and checks it against the invariants and the literal reference algorithm.
    fn run(nodes: &[NodeIn], pin: Option<u32>) -> Layout {
        let l = layout(nodes, pin);
        check(nodes, pin, &l);
        let (lanes, rows, max_lanes) = reference(nodes, pin);
        assert_eq!(l.lanes, lanes, "reference lanes");
        for (i, row) in rows.iter().enumerate() {
            assert_eq!(l.row_edges(i), row.as_slice(), "reference edges of row {i}");
        }
        assert_eq!(l.max_lanes, max_lanes, "reference max_lanes");
        l
    }

    fn dedup(parents: &[u32]) -> Vec<u32> {
        let mut out = Vec::new();
        for &p in parents {
            if !out.contains(&p) {
                out.push(p);
            }
        }
        out
    }

    /// Follows the `k`-th node link of `row` down the graph: the row whose node it ends at, or None when
    /// it leaves the last row (missing parent).
    fn trace(l: &Layout, row: usize, k: usize) -> Option<usize> {
        let mut e = l.row_edges(row)[k];
        let mut r = row;
        loop {
            if e.3 & T != 0 {
                return Some(r + 1);
            }
            if e.3 & O != 0 {
                return None;
            }
            r += 1;
            e = *l.row_edges(r).iter().find(|x| x.3 & F == 0 && x.0 == e.1).expect("line continues");
        }
    }

    /// Structural invariants of any layout (the four rules from the research plus placement rules).
    fn check(nodes: &[NodeIn], pin: Option<u32>, l: &Layout) {
        let n = nodes.len();
        assert_eq!(l.lanes.len(), n);
        assert_eq!(l.colors.len(), n);
        assert_eq!(l.offsets.len(), n + 1);
        assert_eq!(l.offsets[0], 0);
        assert_eq!(l.offsets[n] as usize, l.edges.len());
        assert!(l.offsets.windows(2).all(|w| w[0] <= w[1]));
        let pin = pin.filter(|&p| (p as usize) < n);
        let mut has_child = vec![false; n];
        for (i, node) in nodes.iter().enumerate() {
            for &p in &node.parents {
                if (p as usize) < n && p as usize > i {
                    has_child[p as usize] = true;
                }
            }
        }
        let mut widest = 0u32;
        for i in 0..n {
            let lane = l.lanes[i];
            let edges = l.row_edges(i);
            let last = i + 1 == n;
            let parents = dedup(&nodes[i].parents);
            widest = widest.max(lane as u32 + 1);
            assert_eq!(l.colors[i] as u16, lane % PALETTE, "row {i}: node color");
            assert_eq!(edges.iter().filter(|x| x.3 & F != 0).count(), parents.len(), "row {i}: one link per parent");
            for (k, x) in edges.iter().enumerate() {
                widest = widest.max(x.0 as u32 + 1).max(x.1 as u32 + 1);
                assert_eq!(x.3 & !(D | F | T | M | O), 0, "row {i}: unknown flags");
                assert_eq!(x.3 & O != 0, last, "row {i}: OPEN exactly on the last row");
                if x.3 & F != 0 {
                    // Rule 4: node links start at the node lane; they come first, in parent order.
                    assert_eq!(x.0, lane, "row {i}: node link starts at the node");
                    assert!(k < parents.len(), "row {i}: node links precede passing lines");
                    assert_eq!(x.3 & M != 0, k > 0, "row {i}: MERGE marks 2nd+ parents");
                    assert_eq!(x.3 & D != 0, nodes[i].dashed, "row {i}: node links dashed iff the node is");
                    if k == 0 && x.3 & T == 0 {
                        assert_eq!(x.1, lane, "row {i}: first parent continues in the node lane");
                    }
                    // The renderer runs a merge link down the lane it lands in (`to`, also when that is
                    // the next node) and a first-parent link down the node lane: that lane's color.
                    let drawn_in = if x.3 & M != 0 { x.1 } else { x.0 };
                    assert_eq!(x.2 as u16, drawn_in % PALETTE, "row {i}: link color = the lane it runs down");
                } else {
                    assert_eq!(x.2 as u16, x.0 % PALETTE, "row {i}: passing line color");
                    if x.3 & T == 0 {
                        assert_eq!(x.0, x.1, "row {i}: passing lines never move sideways");
                    }
                }
                if x.3 & T == 0 {
                    assert_eq!(x.2 as u16, x.1 % PALETTE, "row {i}: line color follows its lane");
                }
            }
            if last {
                assert!(edges.iter().all(|x| x.3 & T == 0), "last row: nothing below");
            } else {
                let next = l.lanes[i + 1];
                // Rule 1: TO_NODE edges end at the next node lane and no other edge does.
                for x in edges {
                    if x.3 & T != 0 {
                        assert_eq!(x.1, next, "row {i}: TO_NODE target");
                    } else {
                        assert_ne!(x.1, next, "row {i}: a line crosses the next node");
                    }
                }
                // Rule 2: non-node targets are unique, except a merge link joining a passing line.
                for x in edges.iter().filter(|x| x.3 & T == 0) {
                    let same: Vec<&Edge> = edges.iter().filter(|y| y.3 & T == 0 && y.1 == x.1).collect();
                    if same.len() > 1 {
                        assert_eq!(same.len(), 2, "row {i}: target {} shared", x.1);
                        assert_eq!(same.iter().filter(|y| y.3 & F == 0).count(), 1, "row {i}: join");
                        assert!(same.iter().any(|y| y.3 & M != 0), "row {i}: join by a merge link");
                    }
                }
                // Rule 3: continuity between consecutive rows.
                let mut tos: Vec<u16> = edges.iter().filter(|x| x.3 & T == 0).map(|x| x.1).collect();
                tos.sort_unstable();
                tos.dedup();
                let mut froms: Vec<u16> = l.row_edges(i + 1).iter().filter(|x| x.3 & F == 0).map(|x| x.0).collect();
                froms.sort_unstable();
                let count = froms.len();
                froms.dedup();
                assert_eq!(count, froms.len(), "row {}: duplicate passing lines", i + 1);
                assert_eq!(tos, froms, "row {i}: continuity");
                let lines_in = edges.iter().any(|x| x.3 & T != 0);
                assert_eq!(lines_in, has_child[i + 1], "row {}: lines in iff children", i + 1);
            }
            let above: &[Edge] = if i > 0 { l.row_edges(i - 1) } else { &[] };
            if !has_child[i] && pin != Some(i as u32) {
                // A branch tip takes the lowest lane free at its row.
                let reserved = matches!(pin, Some(p) if (i as u32) < p);
                let taken = |x: u16| (reserved && x == 0) || above.iter().any(|y| y.1 == x);
                let lowest = (0u16..).find(|&x| !taken(x)).unwrap_or_default();
                assert_eq!(lane, lowest, "row {i}: tip lane");
            }
            for x in above.iter().filter(|x| x.3 & T != 0 && x.3 & M == 0) {
                assert!(lane <= x.0, "row {i}: sits in the leftmost lane expecting it");
            }
            if let Some(&p0) = parents.first() {
                if (p0 as usize) < n && p0 as usize > i {
                    assert!(l.lanes[p0 as usize] <= lane, "row {i}: first parent at or left of the node");
                }
            }
            for (k, &p) in parents.iter().enumerate() {
                match trace(l, i, k) {
                    Some(r) => assert_eq!(r as u32, p, "row {i}: link {k} ends at its parent"),
                    None => assert!(p as usize >= n || p as usize <= i, "row {i}: link {k} left the graph"),
                }
            }
        }
        assert_eq!(l.max_lanes, widest, "max_lanes");
        if let Some(p) = pin {
            let p = p as usize;
            assert_eq!(l.lanes[p], 0, "pinned row in lane 0");
            assert!(l.lanes[..p].iter().all(|&x| x != 0), "lane 0 reserved above the pinned row");
            let mut r = p;
            while let Some(&p0) = nodes[r].parents.first() {
                if p0 as usize >= n || p0 as usize <= r {
                    break;
                }
                r = p0 as usize;
                assert_eq!(l.lanes[r], 0, "pinned first-parent chain stays in lane 0");
            }
        }
    }

    /// Straightforward reference layout (slow, for comparison): a 2nd+ parent on the next row
    /// joins the lowest lane heading to it.
    fn reference(nodes: &[NodeIn], pin: Option<u32>) -> (Vec<u16>, Vec<Vec<Edge>>, u32) {
        #[derive(Clone, Copy)]
        struct S {
            expect: u32,
            color: u8,
            dashed: bool,
            visible: bool,
        }
        let n = nodes.len();
        let mut slots: Vec<Option<S>> = Vec::new();
        let mut outs: Vec<(u16, u16, u8, u8)> = Vec::new();
        let mut rows: Vec<Vec<Edge>> = vec![Vec::new(); n];
        let mut lanes = Vec::new();
        let mut max_lanes = 0u32;
        if let Some(p) = pin.filter(|&p| (p as usize) < n) {
            slots.push(Some(S { expect: p, color: 0, dashed: false, visible: false }));
        }
        for i in 0..n {
            let hits: Vec<usize> =
                (0..slots.len()).filter(|&s| matches!(slots[s], Some(x) if x.expect == i as u32)).collect();
            let lane = match hits.first() {
                Some(&h) => h,
                None => match slots.iter().position(|s| s.is_none()) {
                    Some(f) => f,
                    None => {
                        slots.push(None);
                        slots.len() - 1
                    }
                },
            };
            let color = (lane % 8) as u8;
            for (from, slot, col, flags) in outs.drain(..) {
                let ends = hits.contains(&(slot as usize));
                let to = if ends { lane as u16 } else { slot };
                rows[i - 1].push(Edge(from, to, col, flags | if ends { T } else { 0 }));
            }
            let passing: Vec<usize> = (0..slots.len())
                .filter(|&s| matches!(slots[s], Some(x) if x.visible) && !hits.contains(&s))
                .collect();
            for &h in &hits {
                slots[h] = None;
            }
            let kind = if nodes[i].dashed { D } else { 0 };
            let parents = dedup(&nodes[i].parents);
            if let Some(&p0) = parents.first() {
                slots[lane] = Some(S { expect: p0, color, dashed: kind != 0, visible: true });
                outs.push((lane as u16, lane as u16, color, F | kind));
            }
            for &p in parents.iter().skip(1) {
                let next_row = p == i as u32 + 1 && (p as usize) < n;
                let found = (0..slots.len())
                    .filter(|&s| matches!(slots[s], Some(x) if x.expect == p))
                    .min_by_key(|&s| if next_row { (0, s) } else { (s.abs_diff(lane), s) });
                if let Some(s) = found {
                    let x = slots[s].as_mut().expect("slot");
                    if !x.visible {
                        x.visible = true;
                    } else if kind == 0 {
                        x.dashed = false;
                    }
                    outs.push((lane as u16, s as u16, x.color, F | M | kind));
                } else {
                    let s = match (lane + 1..slots.len()).find(|&s| slots[s].is_none()) {
                        Some(s) => s,
                        None => {
                            slots.push(None);
                            slots.len() - 1
                        }
                    };
                    slots[s] = Some(S { expect: p, color: (s % 8) as u8, dashed: kind != 0, visible: true });
                    outs.push((lane as u16, s as u16, (s % 8) as u8, F | M | kind));
                }
            }
            for s in passing {
                let x = slots[s].expect("passing slot");
                outs.push((s as u16, s as u16, x.color, if x.dashed { D } else { 0 }));
            }
            while matches!(slots.last(), Some(None)) {
                slots.pop();
            }
            lanes.push(lane as u16);
            max_lanes = max_lanes.max(lane as u32 + 1);
            for o in &outs {
                max_lanes = max_lanes.max(o.0 as u32 + 1).max(o.1 as u32 + 1);
            }
        }
        for (from, slot, col, flags) in outs.drain(..) {
            rows[n - 1].push(Edge(from, slot, col, flags | O));
        }
        (lanes, rows, max_lanes)
    }

    #[test]
    fn empty_and_single() {
        for pin in [None, Some(0)] {
            let l = run(&[], pin);
            assert!(l.lanes.is_empty() && l.colors.is_empty() && l.edges.is_empty());
            assert_eq!(l.offsets, [0]);
            assert_eq!(l.max_lanes, 0);
        }
        expect(&[c(&[])], None, &[0], &[0], &[&[]], 1);
        expect(&[c(&[])], Some(0), &[0], &[0], &[&[]], 1);
    }

    #[test]
    fn linear_history() {
        let nodes = [c(&[1]), c(&[2]), c(&[])];
        expect(&nodes, None, &[0, 0, 0], &[0, 0, 0], &[&[e(0, 0, 0, F | T)], &[e(0, 0, 0, F | T)], &[]], 1);
    }

    #[test]
    fn fork_converges_at_the_parent_row() {
        // Two branches share base 4; each keeps its own line until the base, never merging early.
        let nodes = [c(&[2]), c(&[3]), c(&[4]), c(&[4]), c(&[])];
        expect(
            &nodes,
            None,
            &[0, 1, 0, 1, 0],
            &[0, 1, 0, 1, 0],
            &[
                &[e(0, 0, 0, F)],
                &[e(1, 1, 1, F), e(0, 0, 0, T)],
                &[e(0, 0, 0, F), e(1, 1, 1, T)],
                &[e(1, 0, 1, F | T), e(0, 0, 0, T)],
                &[],
            ],
            2,
        );
    }

    #[test]
    fn merge_opens_a_new_lane() {
        let nodes = [c(&[1, 2]), c(&[3]), c(&[3]), c(&[])];
        expect(
            &nodes,
            None,
            &[0, 0, 1, 0],
            &[0, 0, 1, 0],
            &[
                &[e(0, 0, 0, F | T), e(0, 1, 1, F | M)],
                &[e(0, 0, 0, F), e(1, 1, 1, T)],
                &[e(1, 0, 1, F | T), e(0, 0, 0, T)],
                &[],
            ],
            2,
        );
    }

    #[test]
    fn merge_joins_the_nearest_lane() {
        // Lanes 0 and 1 both expect row 4; the merge at lane 2 joins lane 1.
        let nodes = [c(&[4]), c(&[4]), c(&[3, 4]), c(&[4]), c(&[])];
        expect(
            &nodes,
            None,
            &[0, 1, 2, 2, 0],
            &[0, 1, 2, 2, 0],
            &[
                &[e(0, 0, 0, F)],
                &[e(1, 1, 1, F), e(0, 0, 0, 0)],
                &[e(2, 2, 2, F | T), e(2, 1, 1, F | M), e(0, 0, 0, 0), e(1, 1, 1, 0)],
                &[e(2, 0, 2, F | T), e(0, 0, 0, T), e(1, 0, 1, T)],
                &[],
            ],
            3,
        );
    }

    #[test]
    fn merge_join_tie_takes_the_lower_lane() {
        // Lanes 0 and 2 both expect row 5; the merge at lane 1 is equally near both and joins lane 0.
        let nodes = [c(&[5]), c(&[3]), c(&[5]), c(&[4, 5]), c(&[5]), c(&[])];
        expect(
            &nodes,
            None,
            &[0, 1, 2, 1, 1, 0],
            &[0, 1, 2, 1, 1, 0],
            &[
                &[e(0, 0, 0, F)],
                &[e(1, 1, 1, F), e(0, 0, 0, 0)],
                &[e(2, 2, 2, F), e(0, 0, 0, 0), e(1, 1, 1, T)],
                &[e(1, 1, 1, F | T), e(1, 0, 0, F | M), e(0, 0, 0, 0), e(2, 2, 2, 0)],
                &[e(1, 0, 1, F | T), e(0, 0, 0, T), e(2, 0, 2, T)],
                &[],
            ],
            3,
        );
    }

    #[test]
    fn merge_into_the_next_row_joins_the_line_of_its_node() {
        // Lanes 0 and 1 head to row 3; the merge at lane 2 links to row 3 too. Lane 1 is nearer, but the
        // link ends at row 3's node in lane 0 and is drawn down lane 0, so it takes lane 0's color.
        let nodes = [c(&[3]), c(&[3]), c(&[4, 3]), c(&[4]), c(&[])];
        expect(
            &nodes,
            None,
            &[0, 1, 2, 0, 0],
            &[0, 1, 2, 0, 0],
            &[
                &[e(0, 0, 0, F)],
                &[e(1, 1, 1, F), e(0, 0, 0, 0)],
                &[e(2, 2, 2, F), e(2, 0, 0, F | M | T), e(0, 0, 0, T), e(1, 0, 1, T)],
                &[e(0, 0, 0, F | T), e(2, 0, 2, T)],
                &[],
            ],
            3,
        );

        // Same with the pinned, dashed WIP line in lane 0: the solid link joins it, so its last gap is solid.
        let nodes = [d(&[3]), c(&[3]), c(&[4, 3]), c(&[4]), c(&[])];
        expect(
            &nodes,
            Some(0),
            &[0, 1, 2, 0, 0],
            &[0, 1, 2, 0, 0],
            &[
                &[e(0, 0, 0, F | D)],
                &[e(1, 1, 1, F), e(0, 0, 0, D)],
                &[e(2, 2, 2, F), e(2, 0, 0, F | M | T), e(0, 0, 0, T), e(1, 0, 1, T)],
                &[e(0, 0, 0, F | T), e(2, 0, 2, T)],
                &[],
            ],
            3,
        );
    }

    #[test]
    fn last_row_merge_into_a_missing_parent_joins_the_nearest_line() {
        // Lanes 0 and 1 head to the missing commit 4 (= rows.len(), i.e. "row + 1" of the last row);
        // the last row's merge link still joins the nearest of them, lane 1.
        let nodes = [c(&[4]), c(&[2]), c(&[4]), c(&[5, 4])];
        expect(
            &nodes,
            None,
            &[0, 1, 1, 2],
            &[0, 1, 1, 2],
            &[
                &[e(0, 0, 0, F)],
                &[e(1, 1, 1, F | T), e(0, 0, 0, 0)],
                &[e(1, 1, 1, F), e(0, 0, 0, 0)],
                &[e(2, 2, 2, F | O), e(2, 1, 1, F | M | O), e(0, 0, 0, O), e(1, 1, 1, O)],
            ],
            3,
        );
    }

    #[test]
    fn octopus_opens_lanes_in_parent_order() {
        let nodes = [c(&[1, 2, 3]), c(&[4]), c(&[4]), c(&[4]), c(&[])];
        expect(
            &nodes,
            None,
            &[0, 0, 1, 2, 0],
            &[0, 0, 1, 2, 0],
            &[
                &[e(0, 0, 0, F | T), e(0, 1, 1, F | M), e(0, 2, 2, F | M)],
                &[e(0, 0, 0, F), e(1, 1, 1, T), e(2, 2, 2, 0)],
                &[e(1, 1, 1, F), e(0, 0, 0, 0), e(2, 2, 2, T)],
                &[e(2, 0, 2, F | T), e(0, 0, 0, T), e(1, 0, 1, T)],
                &[],
            ],
            3,
        );
    }

    #[test]
    fn octopus_mixes_new_and_joined_lanes_and_drops_duplicates() {
        // Row 1 merges [2, 3, 4, 3]: 3 opens lane 2, 4 joins lane 0, the repeated 3 is ignored.
        let nodes = [c(&[4]), c(&[2, 3, 4, 3]), c(&[5]), c(&[5]), c(&[5]), c(&[])];
        expect(
            &nodes,
            None,
            &[0, 1, 1, 2, 0, 0],
            &[0, 1, 1, 2, 0, 0],
            &[
                &[e(0, 0, 0, F)],
                &[e(1, 1, 1, F | T), e(1, 2, 2, F | M), e(1, 0, 0, F | M), e(0, 0, 0, 0)],
                &[e(1, 1, 1, F), e(0, 0, 0, 0), e(2, 2, 2, T)],
                &[e(2, 2, 2, F), e(0, 0, 0, T), e(1, 1, 1, 0)],
                &[e(0, 0, 0, F | T), e(1, 0, 1, T), e(2, 0, 2, T)],
                &[],
            ],
            3,
        );
    }

    #[test]
    fn duplicate_first_parent_is_one_link() {
        let nodes = [c(&[1, 1]), c(&[])];
        expect(&nodes, None, &[0, 0], &[0, 0], &[&[e(0, 0, 0, F | T)], &[]], 1);
    }

    #[test]
    fn colors_wrap_at_the_palette_size() {
        // A 10-parent octopus opens lanes 1..=9; lanes 8 and 9 reuse colors 0 and 1.
        let mut nodes = vec![c(&(1..=10).collect::<Vec<u32>>())];
        nodes.extend((1..=10).map(|_| c(&[])));
        let l = run(&nodes, None);
        let lanes: Vec<u16> = [0].into_iter().chain(0..10).collect();
        assert_eq!(l.lanes, lanes);
        assert_eq!(l.colors, [0, 0, 1, 2, 3, 4, 5, 6, 7, 0, 1]);
        let mut row0 = vec![e(0, 0, 0, F | T)];
        row0.extend((1..10u16).map(|s| e(0, s, (s % 8) as u8, F | M)));
        assert_eq!(l.row_edges(0), row0.as_slice());
        for k in 1..=10u16 {
            let mut want = Vec::new();
            if k < 10 {
                want.push(e(k, k, (k % 8) as u8, T));
                want.extend((k + 1..10).map(|s| e(s, s, (s % 8) as u8, 0)));
            }
            assert_eq!(l.row_edges(k as usize), want.as_slice(), "row {k}");
        }
        assert_eq!(l.max_lanes, 10);
    }

    #[test]
    fn pin_with_wip_row() {
        // WIP (row 0) → HEAD (row 2); a branch tip in between merges HEAD, which turns the WIP line solid.
        let nodes = [d(&[2]), c(&[3, 2]), c(&[3]), c(&[])];
        expect(
            &nodes,
            Some(0),
            &[0, 1, 0, 0],
            &[0, 1, 0, 0],
            &[
                &[e(0, 0, 0, F | D)],
                &[e(1, 1, 1, F), e(1, 0, 0, F | M | T), e(0, 0, 0, T)],
                &[e(0, 0, 0, F | T), e(1, 0, 1, T)],
                &[],
            ],
            2,
        );
    }

    #[test]
    fn pin_head_below_other_tips() {
        // HEAD (row 1) sits on a side branch under the main tip (row 0).
        let nodes = [c(&[2]), c(&[3]), c(&[3]), c(&[])];
        assert_eq!(run(&nodes, None).lanes, [0, 1, 0, 0]);
        expect(
            &nodes,
            Some(1),
            &[1, 0, 1, 0],
            &[1, 0, 1, 0],
            &[&[e(1, 1, 1, F)], &[e(0, 0, 0, F), e(1, 1, 1, T)], &[e(1, 0, 1, F | T), e(0, 0, 0, T)], &[]],
            2,
        );
    }

    #[test]
    fn pin_head_with_a_child_above() {
        // A commit on top of HEAD stays out of lane 0 and bends into HEAD.
        let nodes = [c(&[1]), c(&[2]), c(&[])];
        expect(&nodes, Some(1), &[1, 0, 0], &[1, 0, 0], &[&[e(1, 0, 1, F | T)], &[e(0, 0, 0, F | T)], &[]], 2);
    }

    #[test]
    fn pin_phantom_is_joined_by_a_merge() {
        // Row 0 merges HEAD (row 2): the link joins the reserved lane 0 instead of opening lane 2.
        let nodes = [c(&[1, 2]), c(&[3]), c(&[3]), c(&[])];
        assert_eq!(run(&nodes, None).lanes, [0, 0, 1, 0]);
        expect(
            &nodes,
            Some(2),
            &[1, 1, 0, 0],
            &[1, 1, 0, 0],
            &[
                &[e(1, 1, 1, F | T), e(1, 0, 0, F | M)],
                &[e(1, 1, 1, F), e(0, 0, 0, T)],
                &[e(0, 0, 0, F | T), e(1, 0, 1, T)],
                &[],
            ],
            2,
        );
    }

    #[test]
    fn pin_outside_the_rows_is_ignored() {
        let nodes = [c(&[2]), c(&[2]), c(&[])];
        assert_eq!(layout(&nodes, Some(3)).lanes, layout(&nodes, None).lanes);
        assert_eq!(layout(&nodes, Some(u32::MAX)).edges, layout(&nodes, None).edges);
        assert_eq!(run(&nodes, Some(7)).lanes, [0, 1, 0]);
    }

    #[test]
    fn dashed_wip_line_across_several_rows() {
        let nodes = [d(&[3]), c(&[4]), c(&[4]), c(&[4]), c(&[])];
        expect(
            &nodes,
            Some(0),
            &[0, 1, 2, 0, 0],
            &[0, 1, 2, 0, 0],
            &[
                &[e(0, 0, 0, F | D)],
                &[e(1, 1, 1, F), e(0, 0, 0, D)],
                &[e(2, 2, 2, F), e(0, 0, 0, D | T), e(1, 1, 1, 0)],
                &[e(0, 0, 0, F | T), e(1, 0, 1, T), e(2, 0, 2, T)],
                &[],
            ],
            3,
        );
    }

    #[test]
    fn stash_row_dashed_into_its_base() {
        let nodes = [c(&[2]), d(&[2]), c(&[3]), c(&[])];
        expect(
            &nodes,
            None,
            &[0, 1, 0, 0],
            &[0, 1, 0, 0],
            &[&[e(0, 0, 0, F)], &[e(1, 0, 1, F | D | T), e(0, 0, 0, T)], &[e(0, 0, 0, F | T)], &[]],
            2,
        );
    }

    #[test]
    fn two_stashes_on_one_base() {
        // stash@{0} (row 1) passes stash@{1} (row 2) as a dashed line.
        let nodes = [c(&[3]), d(&[3]), d(&[3]), c(&[4]), c(&[])];
        expect(
            &nodes,
            None,
            &[0, 1, 2, 0, 0],
            &[0, 1, 2, 0, 0],
            &[
                &[e(0, 0, 0, F)],
                &[e(1, 1, 1, F | D), e(0, 0, 0, 0)],
                &[e(2, 0, 2, F | D | T), e(0, 0, 0, T), e(1, 0, 1, D | T)],
                &[e(0, 0, 0, F | T)],
                &[],
            ],
            3,
        );
    }

    #[test]
    fn missing_parents_leave_open_edges() {
        // 8 and 9 are not loaded: their lines run past the last row and never converge.
        let nodes = [c(&[1, 9]), c(&[9]), c(&[8])];
        expect(
            &nodes,
            None,
            &[0, 0, 2],
            &[0, 0, 2],
            &[
                &[e(0, 0, 0, F | T), e(0, 1, 1, F | M)],
                &[e(0, 0, 0, F), e(1, 1, 1, 0)],
                &[e(2, 2, 2, F | O), e(0, 0, 0, O), e(1, 1, 1, O)],
            ],
            3,
        );
    }

    #[test]
    fn merge_joins_a_line_to_a_missing_parent() {
        let nodes = [c(&[9]), c(&[2, 9]), c(&[])];
        expect(
            &nodes,
            None,
            &[0, 1, 1],
            &[0, 1, 1],
            &[&[e(0, 0, 0, F)], &[e(1, 1, 1, F | T), e(1, 0, 0, F | M), e(0, 0, 0, 0)], &[e(0, 0, 0, O)]],
            2,
        );
    }

    #[test]
    fn lane_reuse_after_convergence() {
        // Lanes 0 and 1 converge at row 3; the next tip (row 4) reuses lane 1 instead of opening lane 3.
        let nodes = [c(&[3]), c(&[3]), c(&[5]), c(&[5]), c(&[5]), c(&[])];
        expect(
            &nodes,
            None,
            &[0, 1, 2, 0, 1, 0],
            &[0, 1, 2, 0, 1, 0],
            &[
                &[e(0, 0, 0, F)],
                &[e(1, 1, 1, F), e(0, 0, 0, 0)],
                &[e(2, 2, 2, F), e(0, 0, 0, T), e(1, 0, 1, T)],
                &[e(0, 0, 0, F), e(2, 2, 2, 0)],
                &[e(1, 0, 1, F | T), e(0, 0, 0, T), e(2, 0, 2, T)],
                &[],
            ],
            3,
        );
    }

    #[test]
    fn lane_freed_by_convergence_is_reused_by_a_merge_in_the_same_row() {
        let nodes = [c(&[2]), c(&[2]), c(&[3, 4]), c(&[4]), c(&[])];
        expect(
            &nodes,
            None,
            &[0, 1, 0, 0, 0],
            &[0, 1, 0, 0, 0],
            &[
                &[e(0, 0, 0, F)],
                &[e(1, 0, 1, F | T), e(0, 0, 0, T)],
                &[e(0, 0, 0, F | T), e(0, 1, 1, F | M)],
                &[e(0, 0, 0, F | T), e(1, 0, 1, T)],
                &[],
            ],
            2,
        );
    }

    #[test]
    fn parents_above_the_row_are_treated_as_missing() {
        let nodes = [c(&[0]), c(&[0])];
        expect(&nodes, None, &[0, 1], &[0, 1], &[&[e(0, 0, 0, F)], &[e(1, 1, 1, F | O), e(0, 0, 0, O)]], 2);
    }

    /// Deterministic 64-bit LCG (Knuth MMIX constants).
    struct Lcg(u64);

    impl Lcg {
        fn next(&mut self) -> u32 {
            self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (self.0 >> 33) as u32
        }

        fn below(&mut self, n: u32) -> u32 {
            self.next() % n
        }
    }

    /// Random commit DAG in display order: parents are later rows or missing ids (>= rows). `span` bounds
    /// the first-parent distance (small = many short branches, large = wide graphs).
    fn random_dag(rng: &mut Lcg, rows: u32, span: u32) -> Vec<Vec<u32>> {
        (0..rows)
            .map(|i| {
                let count = match rng.below(100) {
                    0..=4 => 0,
                    5..=74 => 1,
                    75..=94 => 2,
                    _ => 3 + rng.below(4),
                };
                let room = rows - 1 - i;
                let mut parents: Vec<u32> = (0..count)
                    .map(|k| {
                        if room == 0 || rng.below(100) < 3 {
                            rows + rng.below(4)
                        } else {
                            i + 1 + rng.below(room.min(if k == 0 { span } else { span * 8 }))
                        }
                    })
                    .collect();
                if !parents.is_empty() && rng.below(40) == 0 {
                    parents.push(parents[0]);
                }
                parents
            })
            .collect()
    }

    /// Adds a WIP row on top (parent `wip_head`) and stash rows directly above their base rows (newest
    /// first), as `graph_cache` does. Returns the rows and the new index of every base row.
    fn splice(base: &[Vec<u32>], wip_head: Option<u32>, stash_bases: &[u32]) -> (Vec<NodeIn>, Vec<u32>) {
        let m = base.len();
        let mut stashes = vec![0u32; m];
        for &b in stash_bases {
            stashes[b as usize] += 1;
        }
        let mut map = vec![0u32; m];
        let mut next = u32::from(wip_head.is_some());
        for b in 0..m {
            next += stashes[b];
            map[b] = next;
            next += 1;
        }
        let n = next;
        let remap = |p: u32| if (p as usize) < m { map[p as usize] } else { p - m as u32 + n };
        let mut nodes = Vec::with_capacity(n as usize);
        if let Some(h) = wip_head {
            nodes.push(d(&[map[h as usize]]));
        }
        for b in 0..m {
            for _ in 0..stashes[b] {
                nodes.push(d(&[map[b]]));
            }
            nodes.push(NodeIn { parents: base[b].iter().map(|&p| remap(p)).collect(), dashed: false });
        }
        (nodes, map)
    }

    #[test]
    fn random_dags_hold_the_invariants() {
        let mut rng = Lcg(0x5eed_1234_abcd_0001);
        for round in 0..24u32 {
            let rows = 2 + rng.below(700);
            let span = [3, 12, 60][(round % 3) as usize];
            let base = random_dag(&mut rng, rows, span);
            let head = rng.below(rows);
            let stashes: Vec<u32> = (0..rng.below(5)).map(|_| rng.below(rows)).collect();

            let (plain, map) = splice(&base, None, &[]);
            let l = run(&plain, None);
            assert_eq!(layout(&plain, None).edges, l.edges, "deterministic");
            run(&plain, Some(map[head as usize]));
            run(&plain, Some(0));

            let (wip, _) = splice(&base, Some(head), &stashes);
            run(&wip, Some(0));
            run(&wip, None);

            let (stashed, map) = splice(&base, None, &stashes);
            run(&stashed, Some(map[head as usize]));
            run(&stashed, None);
        }
    }

    #[test]
    fn wide_random_dag_holds_the_invariants() {
        let mut rng = Lcg(42);
        let base = random_dag(&mut rng, 3000, 400);
        let (nodes, map) = splice(&base, Some(1500), &[0, 7, 7, 2999]);
        let l = run(&nodes, Some(0));
        assert!(l.max_lanes > 8, "exercises color wrap-around");
        run(&nodes, Some(map[1500]));
    }
}
