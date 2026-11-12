//! Per-repo graph state: loaded commits, synthetic rows (WIP, stashes), layout, decorations.
//! Built by `graph_load`, served in row ranges by `graph_rows`.
//!
//! Commits live in parallel arrays (`Commits`) shared through an `Arc` by later generations that walk
//! the same tips, so toggling the WIP row or the HEAD pin only re-runs the layout. The layout is shared
//! the same way while the rows and the pinned row stay, so a refresh that changes nothing (a window
//! focus) or only labels runs no layout and allocates no edge array. Rows are the commits
//! with a few synthetic rows spliced in (WIP on top, each stash directly above its base): a commit's
//! row is its index plus the number of synthetic rows anchored at or above it, so no per-row mapping
//! is stored.

use crate::error::{AppError, AppResult};
use crate::git::graph::{self, Layout, NodeIn, ParentRef};
use crate::git::{log, refs, search};
use crate::model::{
    Edge, GraphOptions, GraphRow, GraphSummary, RefLabel, RefLabelKind, RefsSnapshot, RepoInfo, RowKind, SearchMode,
    SearchQuery, Tag,
};
use memchr::memmem;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::Duration;

/// Edges whose both ends lie beyond this lane are not sent: the graph column shows at most 64 lanes.
pub const MAX_SENT_LANE: u16 = 63;

/// What a git-backed search asks. Its answer depends only on the walk and the query, so searches with
/// equal keys share one git run (see `WalkSearch`), whichever generation asks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchKey {
    /// `Commits::id` of the walk searched.
    pub walk: u64,
    pub mode: SearchMode,
    /// The query text, trimmed.
    pub text: String,
}

/// One loaded graph generation: commits, synthetic rows, lane layout and ref labels.
pub struct GraphCache {
    pub generation: u64,
    pub opts: GraphOptions,
    pub wip: bool,
    walk: Walk,
    /// WIP and stash rows, ordered by row.
    synthetic: Vec<Synthetic>,
    /// Shared with the generation this one was built from when it lays out the same rows with the same
    /// pin (see `same_layout_input`): on a big history it is the largest buffer here.
    layout: Arc<Layout>,
    /// Ref labels of decorated rows, ordered by row; labels in display order.
    labels: Vec<(u32, Vec<RefLabel>)>,
    head_row: Option<u32>,
    wip_row: Option<u32>,
}

/// The commits of one `git log` walk and what determined them.
struct Walk {
    /// Sorted, deduplicated tip oids fed to `git log`.
    tips: Vec<String>,
    /// `<common dir>/shallow` at read time: deepening a shallow clone changes history, not tips.
    shallow: Vec<u8>,
    commits: Arc<Commits>,
}

impl Walk {
    /// Whether `git log` would print the same commits for these inputs.
    fn same_as(&self, tips: &[String], shallow: &[u8], prev_opts: &GraphOptions, opts: &GraphOptions) -> bool {
        self.tips == tips
            && self.shallow == shallow
            && prev_opts.order == opts.order
            && prev_opts.limit == opts.limit
            && prev_opts.first_parent == opts.first_parent
    }
}

/// A row that is not a loaded commit: the WIP row or a stash.
#[derive(PartialEq)]
struct Synthetic {
    /// Index of the commit this row sits directly above (WIP: 0).
    anchor: u32,
    row: u32,
    kind: RowKind,
    /// Stash oid; empty for the WIP row.
    oid: String,
    /// Stash base, or HEAD for the WIP row (None when HEAD is unborn).
    parent: Option<String>,
    /// N of `stash@{N}` (WIP: 0).
    stash: u32,
    /// `stash@{N}` (WIP: empty).
    name: String,
    subject: String,
    author: String,
    email: String,
    time: i64,
}

impl Synthetic {
    fn to_row(&self) -> GraphRow {
        GraphRow {
            kind: self.kind,
            oid: self.oid.clone(),
            parents: self.parent.iter().cloned().collect(),
            subject: self.subject.clone(),
            author: self.author.clone(),
            email: self.email.clone(),
            time: self.time,
            lane: 0,
            color: 0,
            edges: Vec::new(),
            refs: Vec::new(),
            title: None,
        }
    }
}

impl GraphCache {
    /// Build a new cache. `previous` may be reused (skip `git log`) when options and ref tips are unchanged,
    /// and its layout too when the rows and the pinned row are (see `assemble`).
    pub fn build(
        info: &RepoInfo,
        opts: &GraphOptions,
        wip: bool,
        generation: u64,
        previous: Option<&GraphCache>,
        refs_snapshot: Option<Arc<RefsSnapshot>>,
        cancel: Option<Arc<AtomicBool>>,
    ) -> AppResult<GraphCache> {
        // A snapshot the frontend fetched a moment ago (repo_refs) saves ~7 git processes per refresh.
        let snap = match refs_snapshot {
            Some(s) => s,
            None => Arc::new(refs::snapshot(info)?),
        };
        let tips = select_tips(&snap, opts);
        let shallow = std::fs::read(Path::new(&info.common_dir).join("shallow")).unwrap_or_default();
        // Stash rows take their author from the snapshot: the walk is the only git process here.
        let reuse = previous.filter(|p| p.walk.same_as(&tips, &shallow, &p.opts, opts));
        let commits = match reuse {
            Some(p) => p.walk.commits.clone(),
            None => Arc::new(read_walk(info, &tips, opts, cancel)?),
        };
        let walk = Walk { tips, shallow, commits };
        Ok(Self::assemble(info.is_bare, opts, wip, generation, &snap, walk, reuse))
    }

    /// Rows, layout and decorations over the commits of `walk` (no git access). `prev` is the generation
    /// `walk` was taken from, if any: its layout is reused when it lays out the same rows with the same pin.
    fn assemble(
        is_bare: bool,
        opts: &GraphOptions,
        wip: bool,
        generation: u64,
        snap: &RefsSnapshot,
        walk: Walk,
        prev: Option<&GraphCache>,
    ) -> GraphCache {
        let head_oid = snap.head.oid.as_deref();
        let mut synthetic = Vec::new();
        if wip && !is_bare {
            synthetic.push(Synthetic {
                anchor: 0,
                row: 0,
                kind: RowKind::Wip,
                oid: String::new(),
                parent: head_oid.map(String::from),
                stash: 0,
                name: String::new(),
                subject: String::new(),
                author: String::new(),
                email: String::new(),
                time: 0,
            });
        }
        if opts.include_stashes {
            for s in &snap.stashes {
                // Stashes whose base is not loaded have nowhere to go.
                let Some(base) = walk.commits.find(&s.base) else { continue };
                synthetic.push(Synthetic {
                    anchor: base,
                    row: 0,
                    kind: RowKind::Stash,
                    oid: s.oid.clone(),
                    parent: Some(s.base.clone()),
                    stash: s.index,
                    name: s.name.clone(),
                    subject: s.message.clone(),
                    author: s.author.clone(),
                    email: s.email.clone(),
                    time: s.time,
                });
            }
        }
        // WIP is row 0; several stashes on one base: newest (stash@{0}) topmost.
        synthetic.sort_by_key(|s| (s.anchor, s.kind != RowKind::Wip, s.stash));
        for (k, s) in synthetic.iter_mut().enumerate() {
            s.row = s.anchor + k as u32;
        }
        let mut cache = GraphCache {
            generation,
            opts: opts.clone(),
            wip,
            walk,
            synthetic,
            layout: Arc::default(),
            labels: Vec::new(),
            head_row: None,
            wip_row: None,
        };
        cache.wip_row = cache.synthetic.first().filter(|s| s.kind == RowKind::Wip).map(|s| s.row);
        cache.head_row = head_oid.and_then(|o| cache.walk.commits.find(o)).map(|c| cache.commit_row(c));
        // Every window focus and refs event reloads the graph; when that changed nothing (or only labels)
        // the layout pass over all rows, and its edge array next to the live one, are skipped.
        cache.layout = match prev.filter(|p| p.same_layout_input(&cache)) {
            Some(p) => Arc::clone(&p.layout),
            None => Arc::new(graph::layout(&cache.layout_nodes(), cache.pin())),
        };
        let stash_rows: Vec<(u32, &str)> =
            cache.synthetic.iter().filter(|s| s.kind == RowKind::Stash).map(|s| (s.row, s.name.as_str())).collect();
        let labels = decorate(snap, opts, |oid| cache.walk.commits.find(oid).map(|c| cache.commit_row(c)), &stash_rows);
        cache.labels = labels;
        cache
    }

    /// Whether `other` serves exactly what this graph serves (rows, edges, labels, summary and search
    /// answers): the same walk, synthetic rows, labels and pinned row. The layout is a function of the
    /// rows and the pin, so it is not compared.
    pub fn same_graph(&self, other: &GraphCache) -> bool {
        Arc::ptr_eq(&self.walk.commits, &other.walk.commits)
            && self.walk.tips == other.walk.tips
            && self.synthetic == other.synthetic
            && self.head_row == other.head_row
            && self.wip_row == other.wip_row
            && self.pin() == other.pin()
            && same_labels(&self.labels, &other.labels)
    }

    /// Whether `graph::layout` gets the same input for `other` as it got for this graph, so `other` can
    /// share this layout: the same walk (`layout_nodes` reads nothing else of it), the same synthetic
    /// rows at the same places with the same parents (the WIP flag, the HEAD under the WIP row, the
    /// stash rows), and the same pinned row (HEAD can move between two tips without changing the walk).
    fn same_layout_input(&self, other: &GraphCache) -> bool {
        let same_node =
            |a: &Synthetic, b: &Synthetic| a.anchor == b.anchor && a.kind == b.kind && a.parent == b.parent;
        Arc::ptr_eq(&self.walk.commits, &other.walk.commits)
            && self.synthetic.len() == other.synthetic.len()
            && self.synthetic.iter().zip(&other.synthetic).all(|(a, b)| same_node(a, b))
            && self.pin() == other.pin()
    }

    /// Key of query `q` over this generation's walk.
    pub fn search_key(&self, q: &SearchQuery) -> SearchKey {
        SearchKey { walk: self.walk.commits.id, mode: q.mode, text: q.text.trim().to_string() }
    }

    /// Row the layout keeps in lane 0 (the WIP row, else HEAD's) when the HEAD lane is pinned.
    fn pin(&self) -> Option<u32> {
        if self.opts.pin_head_lane {
            self.wip_row.or(self.head_row)
        } else {
            None
        }
    }

    /// Row count, width and special rows of this generation.
    pub fn summary(&self) -> GraphSummary {
        GraphSummary {
            generation: self.generation,
            total: self.total(),
            max_lanes: self.layout.max_lanes,
            head_row: self.head_row,
            wip_row: self.wip_row,
            truncated: self.walk.commits.truncated,
        }
    }

    /// Rows [start, end), clamped to the available range.
    pub fn rows(&self, start: u32, end: u32) -> Vec<GraphRow> {
        let total = self.total();
        let start = start.min(total);
        let end = end.clamp(start, total);
        let commits = &*self.walk.commits;
        // Synthetic rows and labels before `row`, advanced in step with it.
        let mut k = self.synthetic.partition_point(|s| s.row < start);
        let mut li = self.labels.partition_point(|(r, _)| *r < start);
        let mut out = Vec::with_capacity((end - start) as usize);
        for row in start..end {
            let mut r = match self.synthetic.get(k) {
                Some(s) if s.row == row => {
                    k += 1;
                    s.to_row()
                }
                _ => commits.to_row((row - k as u32) as usize),
            };
            let i = row as usize;
            r.lane = self.layout.lanes.get(i).copied().unwrap_or(0);
            r.color = self.layout.colors.get(i).copied().map_or(0, u16::from);
            r.edges = sent_edges(&self.layout, i);
            if let Some((_, labels)) = self.labels.get(li).filter(|(lr, _)| *lr == row) {
                r.refs = labels.clone();
                li += 1;
            }
            out.push(r);
        }
        out
    }

    /// Row index of a full oid ('' = WIP row).
    pub fn find(&self, oid: &str) -> Option<u32> {
        if oid.is_empty() {
            return self.wip_row;
        }
        if let Some(c) = self.walk.commits.find(oid) {
            return Some(self.commit_row(c));
        }
        self.synthetic.iter().find(|s| s.kind == RowKind::Stash && s.oid == oid).map(|s| s.row)
    }

    /// Matching rows, ascending. Quick mode is in-memory; other modes run git (see `search`).
    pub fn search(&self, info: &RepoInfo, q: &SearchQuery, cancel: Option<Arc<AtomicBool>>) -> AppResult<Vec<u32>> {
        let text = q.text.trim();
        if text.is_empty() {
            return Ok(Vec::new());
        }
        if q.mode == SearchMode::Quick {
            return Ok(self.quick_search(text));
        }
        let commits = &*self.walk.commits;
        // A graph reload re-runs the active search. A reload that keeps the walk (WIP / pin / label
        // changes) gets the same answer, so it takes the walk's last answer, or joins the git run still
        // computing it, instead of starting git again (a pickaxe can take many seconds).
        let hits = commits.search.hits(q.mode, text, cancel.as_deref(), || {
            let oids = search::git_search(
                Path::new(&info.path),
                &self.search_revs(),
                self.opts.order,
                self.opts.limit,
                self.opts.first_parent,
                q.mode,
                text,
                cancel.clone(),
            )?;
            let mut hits: Vec<u32> = oids.iter().filter_map(|o| commits.find(o)).collect();
            hits.sort_unstable();
            hits.dedup();
            Ok(hits)
        })?;
        // Commit rows grow with the commit index, so the rows stay ascending.
        Ok(hits.iter().map(|&c| self.commit_row(c)).collect())
    }

    fn total(&self) -> u32 {
        (self.walk.commits.len() + self.synthetic.len()) as u32
    }

    /// Row of commit index `c`: synthetic rows anchored at or above it come first.
    fn commit_row(&self, c: u32) -> u32 {
        c + self.synthetic.partition_point(|s| s.anchor <= c) as u32
    }

    /// Layout input, one node per row. Parents are row indices; a parent that is not loaded gets an id
    /// `>= rows`, the same id for the same missing commit.
    fn layout_nodes(&self) -> Vec<NodeIn> {
        let commits = &*self.walk.commits;
        let rows = self.total();
        let mut nodes = Vec::with_capacity(rows as usize);
        let mut syn = self.synthetic.iter().peekable();
        for c in 0..commits.len() as u32 {
            while let Some(s) = syn.next_if(|s| s.anchor == c) {
                nodes.push(self.synthetic_node(s, rows));
            }
            let row = nodes.len() as u32;
            let parents = commits.parent_refs(c as usize).iter().map(|&v| self.parent_ref(v, row, rows)).collect();
            nodes.push(NodeIn { parents, dashed: false });
        }
        // The WIP row of an empty history.
        for s in syn {
            nodes.push(self.synthetic_node(s, rows));
        }
        nodes
    }

    /// Layout ref of parent value `v` (see `Commits::parents`) of the commit at `row`.
    fn parent_ref(&self, v: u32, row: u32, rows: u32) -> ParentRef {
        let commits = &*self.walk.commits;
        let n = commits.len() as u32;
        if v >= n {
            return rows + (v - n);
        }
        let r = self.commit_row(v);
        // git lists children before parents; a parent above its child would break the layout, so such a
        // commit is drawn as not loaded (ids past the missing list and the WIP's own missing HEAD).
        if r > row {
            r
        } else {
            rows + commits.missing.len() as u32 + 1 + v
        }
    }

    fn synthetic_node(&self, s: &Synthetic, rows: u32) -> NodeIn {
        let commits = &*self.walk.commits;
        let parents = s.parent.as_deref().map(|p| match commits.find(p) {
            // Stash bases are loaded by construction; the WIP row sits above every commit.
            Some(c) => self.commit_row(c),
            None => rows + commits.missing_index(p).unwrap_or(commits.missing.len()) as u32,
        });
        NodeIn { parents: parents.into_iter().collect(), dashed: true }
    }

    /// `--stdin` revisions for git searches, covering exactly the loaded commits: the loaded tips minus
    /// the history behind parents cut off by the commit limit. (Children precede parents, so nothing
    /// loaded is reachable from an unloaded tip or a cut-off parent. A first-parent walk orders its
    /// commits along first parents only: there, with several tips and the limit hit, a loaded commit
    /// that a cut-off parent reaches through a merge's other parent would be left out of a search.)
    fn search_revs(&self) -> Vec<String> {
        let commits = &*self.walk.commits;
        let missing = &commits.missing;
        let mut revs = Vec::with_capacity(self.walk.tips.len() + missing.len());
        revs.extend(self.walk.tips.iter().filter(|t| commits.find(t).is_some()).cloned());
        revs.extend((0..missing.len()).map(|k| format!("^{}", missing.get(k))));
        revs
    }

    /// Case-insensitive substring over subject, pull request title, author, email and ref label names,
    /// plus oid prefixes (text of at least 4 hex digits).
    fn quick_search(&self, text: &str) -> Vec<u32> {
        let mut needle = String::new();
        lower_into(text, &mut needle);
        let finder = memmem::Finder::new(needle.as_bytes());
        let prefix =
            (text.len() >= 4 && text.bytes().all(|b| b.is_ascii_hexdigit())).then(|| text.to_ascii_lowercase());
        let oid_hit = |oid: &str| prefix.as_deref().is_some_and(|p| oid.starts_with(p));
        let mut buf = String::new();
        let mut hit = |hay: &str| {
            lower_into(hay, &mut buf);
            finder.find(buf.as_bytes()).is_some()
        };
        let commits = &*self.walk.commits;
        let person_hit: Vec<bool> =
            (0..commits.names.len()).map(|p| hit(commits.names.get(p)) || hit(commits.emails.get(p))).collect();
        let mut rows = Vec::new();
        for i in 0..commits.len() {
            if person_hit[commits.author[i] as usize] || oid_hit(commits.oids.get(i)) || hit(commits.subjects.get(i)) {
                rows.push(self.commit_row(i as u32));
            }
        }
        // A merge shows its pull request's title: it is found by it too.
        for (c, title) in &commits.titles {
            if hit(title) {
                rows.push(self.commit_row(*c));
            }
        }
        for s in self.synthetic.iter().filter(|s| s.kind == RowKind::Stash) {
            if oid_hit(&s.oid) || hit(&s.subject) || hit(&s.author) || hit(&s.email) {
                rows.push(s.row);
            }
        }
        for (row, labels) in &self.labels {
            if labels.iter().any(|l| hit(&l.name)) {
                rows.push(*row);
            }
        }
        rows.sort_unstable();
        rows.dedup();
        rows
    }
}

/// The commits of a walk over `tips`, with the pull request titles of its merges when it follows
/// first parents (a second `git log`, beside the walk). Titles that cannot be read are left out; a
/// cancelled read fails the load, as a cancelled walk does.
fn read_walk(
    info: &RepoInfo,
    tips: &[String],
    opts: &GraphOptions,
    cancel: Option<Arc<AtomicBool>>,
) -> AppResult<Commits> {
    let cwd = Path::new(&info.path);
    let (buf, bodies) = std::thread::scope(|s| {
        let bodies = opts.first_parent.then(|| {
            let cancel = cancel.clone();
            s.spawn(move || log::read_merge_bodies(cwd, tips, opts.order, opts.limit, cancel))
        });
        let buf = log::read_raw(cwd, tips, opts.order, opts.limit, opts.first_parent, cancel.clone());
        let bodies =
            bodies.map(|h| h.join().unwrap_or_else(|_| Err(AppError::msg("git worker thread panicked"))));
        (buf, bodies)
    });
    let mut commits = Commits::parse(&buf?, opts.limit, opts.first_parent);
    match bodies {
        Some(Ok(b)) => commits.set_titles(&b),
        Some(Err(e)) if is_set(cancel.as_deref()) => return Err(e),
        Some(Err(e)) => eprintln!("[git-viewer] pull request titles failed: {e}"),
        None => {}
    }
    Ok(commits)
}

/// Edges of `row` worth sending: at least one end within the first 64 lanes.
fn sent_edges(layout: &Layout, row: usize) -> Vec<Edge> {
    if row + 1 >= layout.offsets.len() {
        return Vec::new();
    }
    layout.row_edges(row).iter().filter(|e| e.0 <= MAX_SENT_LANE || e.1 <= MAX_SENT_LANE).copied().collect()
}

/// Tags the graph walks and labels: those peeling to a commit. A tag on a tree or blob (linux's
/// `v2.6.11-tree`) has no row and is left to the sidebar.
fn commit_tags(snap: &RefsSnapshot) -> impl Iterator<Item = &Tag> {
    snap.tags.iter().filter(|t| t.target_type == "commit")
}

/// Tip oids for the walk, sorted and deduplicated:
/// - `only_refs` set: those refs that still exist;
/// - else locals, remotes / tags when included, other worktrees' HEADs, minus `hidden_refs`;
/// - plus HEAD with `include_head`, whatever is hidden (the simple view leaves it out: it follows its
///   branch alone);
/// - plus stash bases when stashes are included (never the stash commits: their index and untracked
///   helper commits would become rows).
///
/// Only commit tags count (see `commit_tags`).
fn select_tips(snap: &RefsSnapshot, opts: &GraphOptions) -> Vec<String> {
    let mut tips: Vec<&str> = Vec::new();
    if opts.only_refs.is_empty() {
        let hidden: HashSet<&str> = opts.hidden_refs.iter().map(String::as_str).collect();
        let keep = |full: &str| !hidden.contains(full);
        tips.extend(snap.locals.iter().filter(|b| keep(&b.full_name)).map(|b| b.oid.as_str()));
        if opts.include_remotes {
            tips.extend(snap.remotes.iter().filter(|b| keep(&b.full_name)).map(|b| b.oid.as_str()));
        }
        if opts.include_tags {
            tips.extend(commit_tags(snap).filter(|t| keep(&t.full_name)).map(|t| t.oid.as_str()));
        }
        for w in snap.worktrees.iter().filter(|w| !w.is_current) {
            let Some(head) = w.head.as_deref() else { continue };
            // `branch` is the name without `refs/heads/` (a branch may itself be called `refs/x`); a
            // HEAD symref outside `refs/heads/` keeps its full name.
            let branch_hidden = !w.detached
                && w.branch.as_deref().is_some_and(|b| {
                    !keep(&format!("refs/heads/{b}")) || (b.starts_with("refs/") && !keep(b))
                });
            if !branch_hidden {
                tips.push(head);
            }
        }
    } else {
        let only: HashSet<&str> = opts.only_refs.iter().map(String::as_str).collect();
        tips.extend(snap.locals.iter().filter(|b| only.contains(b.full_name.as_str())).map(|b| b.oid.as_str()));
        tips.extend(snap.remotes.iter().filter(|b| only.contains(b.full_name.as_str())).map(|b| b.oid.as_str()));
        tips.extend(commit_tags(snap).filter(|t| only.contains(t.full_name.as_str())).map(|t| t.oid.as_str()));
    }
    if opts.include_head {
        tips.extend(snap.head.oid.as_deref());
    }
    if opts.include_stashes {
        tips.extend(snap.stashes.iter().map(|s| s.base.as_str()));
    }
    tips.retain(|t| !t.is_empty());
    tips.sort_unstable();
    tips.dedup();
    tips.into_iter().map(String::from).collect()
}

/// Display order inside a row: head, current local, other locals, remotes, tags, stash, worktree.
fn label_rank(l: &RefLabel) -> u8 {
    match l.kind {
        RefLabelKind::Head => 0,
        RefLabelKind::Local if l.current => 1,
        RefLabelKind::Local => 2,
        RefLabelKind::Remote => 3,
        RefLabelKind::Tag => 4,
        RefLabelKind::Stash => 5,
        RefLabelKind::Worktree => 6,
    }
}

/// Ref labels per row (rows ascending, labels in display order, by name within a kind). `row_of` maps
/// a commit oid to its row; `stash_rows` are the stash rows with their `stash@{N}` names. Labels follow
/// the walk filters (hidden refs and excluded kinds are unlabelled); a soloed ref is always labelled.
/// Tags not peeling to a commit never are.
fn decorate(
    snap: &RefsSnapshot,
    opts: &GraphOptions,
    row_of: impl Fn(&str) -> Option<u32>,
    stash_rows: &[(u32, &str)],
) -> Vec<(u32, Vec<RefLabel>)> {
    let only: HashSet<&str> = opts.only_refs.iter().map(String::as_str).collect();
    let hidden: HashSet<&str> = opts.hidden_refs.iter().map(String::as_str).collect();
    let shown = |full: &str, included: bool| only.contains(full) || (included && !hidden.contains(full));
    let mut all: Vec<(u32, RefLabel)> = Vec::new();
    let mut add = |oid: &str, kind: RefLabelKind, name: &str, full_name: &str, current: bool| {
        if let Some(row) = row_of(oid) {
            all.push((row, RefLabel { kind, name: name.to_string(), full_name: full_name.to_string(), current }));
        }
    };
    if snap.head.detached {
        if let Some(oid) = snap.head.oid.as_deref() {
            add(oid, RefLabelKind::Head, "HEAD", "HEAD", true);
        }
    }
    for b in snap.locals.iter().filter(|b| shown(&b.full_name, true)) {
        add(&b.oid, RefLabelKind::Local, &b.name, &b.full_name, b.is_head);
    }
    for b in snap.remotes.iter().filter(|b| shown(&b.full_name, opts.include_remotes)) {
        add(&b.oid, RefLabelKind::Remote, &b.name, &b.full_name, false);
    }
    for t in commit_tags(snap).filter(|t| shown(&t.full_name, opts.include_tags)) {
        add(&t.oid, RefLabelKind::Tag, &t.name, &t.full_name, false);
    }
    for w in snap.worktrees.iter().filter(|w| !w.is_current && w.detached && !w.bare) {
        if let Some(head) = w.head.as_deref() {
            let folder =
                Path::new(&w.path).file_name().map_or_else(|| w.path.clone(), |n| n.to_string_lossy().into_owned());
            add(head, RefLabelKind::Worktree, &folder, &w.path, false);
        }
    }
    for &(row, name) in stash_rows {
        all.push((
            row,
            RefLabel { kind: RefLabelKind::Stash, name: name.to_string(), full_name: name.to_string(), current: false },
        ));
    }
    all.sort_by(|a, b| (a.0, label_rank(&a.1)).cmp(&(b.0, label_rank(&b.1))).then_with(|| a.1.name.cmp(&b.1.name)));
    let mut out: Vec<(u32, Vec<RefLabel>)> = Vec::new();
    for (row, label) in all {
        match out.last_mut() {
            Some((r, v)) if *r == row => v.push(label),
            _ => out.push((row, vec![label])),
        }
    }
    out
}

/// Whether two `decorate` results are equal (`RefLabel` has no `PartialEq`).
fn same_labels(a: &[(u32, Vec<RefLabel>)], b: &[(u32, Vec<RefLabel>)]) -> bool {
    let same = |x: &RefLabel, y: &RefLabel| {
        x.kind == y.kind && x.current == y.current && x.name == y.name && x.full_name == y.full_name
    };
    a.len() == b.len()
        && a.iter().zip(b).all(|((ra, la), (rb, lb))| {
            ra == rb && la.len() == lb.len() && la.iter().zip(lb).all(|(x, y)| same(x, y))
        })
}

/// Lowercase `s` into `out` (Unicode-aware; ASCII fast path).
fn lower_into(s: &str, out: &mut String) {
    out.clear();
    if s.is_ascii() {
        out.push_str(s);
        out.make_ascii_lowercase();
    } else {
        out.extend(s.chars().flat_map(char::to_lowercase));
    }
}

// ───────────────────────────── Commit store ─────────────────────────────

/// Strings stored back to back in one buffer: no per-string allocation.
#[derive(Default)]
struct StrPool {
    buf: String,
    ends: Vec<usize>,
}

impl StrPool {
    fn with_capacity(items: usize, bytes: usize) -> StrPool {
        StrPool { buf: String::with_capacity(bytes), ends: Vec::with_capacity(items) }
    }

    fn push(&mut self, b: &[u8]) {
        self.buf.push_str(&String::from_utf8_lossy(b));
        self.ends.push(self.buf.len());
    }

    fn len(&self) -> usize {
        self.ends.len()
    }

    fn get(&self, i: usize) -> &str {
        let start = if i == 0 { 0 } else { self.ends[i - 1] };
        &self.buf[start..self.ends[i]]
    }

    fn shrink(&mut self) {
        self.buf.shrink_to_fit();
        self.ends.shrink_to_fit();
    }
}

/// Source of `Commits::id`.
static NEXT_WALK_ID: AtomicU64 = AtomicU64::new(1);

/// Commits of one walk (children first) as parallel arrays.
struct Commits {
    /// Identifies the walk; the generations that reuse it share it.
    id: u64,
    oids: StrPool,
    subjects: StrPool,
    /// Interned authors: person `p` is (`names[p]`, `emails[p]`).
    names: StrPool,
    emails: StrPool,
    author: Vec<u32>,
    time: Vec<i64>,
    /// Commit `i` owns `parents[parent_ends[i - 1]..parent_ends[i]]`. A value below `len()` is a commit
    /// index; `len() + k` is `missing[k]`, a parent outside the walk.
    parents: Vec<u32>,
    parent_ends: Vec<u32>,
    missing: StrPool,
    index: OidIndex,
    /// Pull request titles of merge commits, by commit index, ascending (`set_titles`; first-parent
    /// walks only).
    titles: Vec<(u32, String)>,
    /// The walk stopped at the commit limit.
    truncated: bool,
    /// Git-backed searches over this walk, shared by the generations that reuse it.
    search: WalkSearch,
}

impl Commits {
    /// Store the records of `git log` output (see `log::records`), read with commit limit `limit`.
    /// `first_parent`: a first-parent walk's output, whose `%P` still lists every parent of a merge:
    /// only the first is kept, the others are not in the walk (and must not count as cut off by it).
    fn parse(buf: &[u8], limit: u32, first_parent: bool) -> Commits {
        let hint = log::record_count_hint(buf);
        let mut oids = StrPool::with_capacity(hint, hint * 40);
        let mut subjects = StrPool::with_capacity(hint, 0);
        let mut names = StrPool::default();
        let mut emails = StrPool::default();
        let mut people: HashMap<(&[u8], &[u8]), u32> = HashMap::new();
        let mut author = Vec::with_capacity(hint);
        let mut time = Vec::with_capacity(hint);
        let mut parent_fields: Vec<&[u8]> = Vec::with_capacity(hint);
        for rec in log::records(buf) {
            oids.push(rec.oid);
            subjects.push(rec.subject);
            let next = people.len() as u32;
            let p = *people.entry((rec.author, rec.email)).or_insert_with(|| {
                names.push(rec.author);
                emails.push(rec.email);
                next
            });
            author.push(p);
            time.push(rec.time);
            parent_fields.push(rec.parents);
        }
        let index = OidIndex::build(&oids);
        let n = oids.len() as u32;
        let mut parents = Vec::with_capacity(parent_fields.len() + parent_fields.len() / 4);
        let mut parent_ends = Vec::with_capacity(parent_fields.len());
        let mut missing = StrPool::default();
        let mut missing_ids: HashMap<&[u8], u32> = HashMap::new();
        let per_commit = if first_parent { 1 } else { usize::MAX };
        for field in parent_fields {
            for p in field.split(|&b| b == b' ').filter(|p| !p.is_empty()).take(per_commit) {
                let v = match index.get(&oids, p) {
                    Some(c) => c,
                    None => {
                        let k = missing_ids.len() as u32;
                        n + *missing_ids.entry(p).or_insert_with(|| {
                            missing.push(p);
                            k
                        })
                    }
                };
                parents.push(v);
            }
            parent_ends.push(parents.len() as u32);
        }
        for pool in [&mut oids, &mut subjects, &mut names, &mut emails] {
            pool.shrink();
        }
        author.shrink_to_fit();
        time.shrink_to_fit();
        parents.shrink_to_fit();
        Commits {
            id: NEXT_WALK_ID.fetch_add(1, Ordering::Relaxed),
            truncated: limit > 0 && n >= limit,
            oids,
            subjects,
            names,
            emails,
            author,
            time,
            parents,
            parent_ends,
            missing,
            index,
            titles: Vec::new(),
            search: WalkSearch::default(),
        }
    }

    /// Take the pull request titles of the merges in `bodies` (`log::read_merge_bodies` output):
    /// `log::pr_title` of each merge's subject and body, for the merges this walk holds.
    fn set_titles(&mut self, bodies: &[u8]) {
        let mut titles: Vec<(u32, String)> = log::merge_bodies(bodies)
            .into_iter()
            .filter_map(|(oid, body)| {
                let c = self.index.get(&self.oids, oid)?;
                let body = String::from_utf8_lossy(body);
                log::pr_title(self.subjects.get(c as usize), &body).map(|t| (c, t.to_string()))
            })
            .collect();
        titles.sort_unstable_by_key(|t| t.0);
        titles.dedup_by_key(|t| t.0);
        titles.shrink_to_fit();
        self.titles = titles;
    }

    /// Pull request title of commit `i` (see `set_titles`).
    fn title(&self, i: usize) -> Option<&str> {
        let k = self.titles.binary_search_by_key(&(i as u32), |t| t.0).ok()?;
        Some(&self.titles[k].1)
    }

    fn len(&self) -> usize {
        self.oids.len()
    }

    fn find(&self, oid: &str) -> Option<u32> {
        self.index.get(&self.oids, oid.as_bytes())
    }

    fn missing_index(&self, oid: &str) -> Option<usize> {
        (0..self.missing.len()).find(|&k| self.missing.get(k) == oid)
    }

    fn parent_refs(&self, i: usize) -> &[u32] {
        let start = if i == 0 { 0 } else { self.parent_ends[i - 1] as usize };
        &self.parents[start..self.parent_ends[i] as usize]
    }

    /// Oid of parent value `v`.
    fn parent_oid(&self, v: u32) -> &str {
        let n = self.len();
        let v = v as usize;
        if v < n {
            self.oids.get(v)
        } else {
            self.missing.get(v - n)
        }
    }

    fn to_row(&self, i: usize) -> GraphRow {
        let p = self.author[i] as usize;
        GraphRow {
            kind: RowKind::Commit,
            oid: self.oids.get(i).to_string(),
            parents: self.parent_refs(i).iter().map(|&v| self.parent_oid(v).to_string()).collect(),
            subject: self.subjects.get(i).to_string(),
            author: self.names.get(p).to_string(),
            email: self.emails.get(p).to_string(),
            time: self.time[i],
            lane: 0,
            color: 0,
            edges: Vec::new(),
            refs: Vec::new(),
            title: self.title(i).map(String::from),
        }
    }
}

// ───────────────────────────── Git searches of a walk ─────────────────────────────

/// How often a search waiting for another search's git run checks its own cancel flag.
const JOIN_POLL: Duration = Duration::from_millis(10);

/// The git-backed searches over one walk. A query's answer depends only on the walk, so one git run
/// serves every generation that reuses it: a search that starts while git runs the same query (a
/// graph reload re-running the active search) waits for that run instead of starting another, and
/// the last answer is kept for later ones.
#[derive(Default)]
struct WalkSearch {
    slot: Mutex<Option<SearchSlot>>,
}

enum SearchSlot {
    Running(Arc<SearchRun>),
    /// Matching commit indices, ascending.
    Done { mode: SearchMode, text: String, hits: Arc<[u32]> },
}

/// One git run of a query. Searches that join it wait for `answer`.
struct SearchRun {
    mode: SearchMode,
    text: String,
    answer: Mutex<Option<RunAnswer>>,
    done: Condvar,
}

#[derive(Clone)]
enum RunAnswer {
    Hits(Arc<[u32]>),
    Failed(String),
    /// The run was cancelled. A search that joined it with a flag still clear runs git itself.
    Abandoned,
}

impl SearchRun {
    /// The answer once the run ends, or "cancelled" as soon as `cancel` is set.
    fn wait(&self, cancel: Option<&AtomicBool>) -> AppResult<RunAnswer> {
        let mut answer = self.answer.lock().unwrap();
        loop {
            if let Some(a) = answer.as_ref() {
                return Ok(a.clone());
            }
            if is_set(cancel) {
                return Err(AppError::msg("cancelled"));
            }
            answer = self.done.wait_timeout(answer, JOIN_POLL).unwrap().0;
        }
    }
}

impl WalkSearch {
    /// Commit indices matching `mode` / `text`: the kept answer, the answer of the run in progress, or
    /// that of a new run of `git` (which honours `cancel` itself).
    fn hits(
        &self,
        mode: SearchMode,
        text: &str,
        cancel: Option<&AtomicBool>,
        git: impl FnOnce() -> AppResult<Vec<u32>>,
    ) -> AppResult<Arc<[u32]>> {
        let run = loop {
            let mut slot = self.slot.lock().unwrap();
            match &*slot {
                Some(SearchSlot::Done { mode: m, text: t, hits }) if *m == mode && t == text => {
                    return Ok(hits.clone());
                }
                Some(SearchSlot::Running(run)) if run.mode == mode && run.text == text => {
                    let run = run.clone();
                    drop(slot);
                    match run.wait(cancel)? {
                        RunAnswer::Hits(hits) => return Ok(hits),
                        RunAnswer::Failed(e) => return Err(AppError::msg(e)),
                        // Its owner has already left the slot: look again.
                        RunAnswer::Abandoned => continue,
                    }
                }
                // Superseded (before it started, or while it waited for a run that was then abandoned): a
                // run of its own would only take the slot from the search that superseded it, whose
                // answer would then be neither joined nor kept.
                _ if is_set(cancel) => return Err(AppError::msg("cancelled")),
                _ => {
                    let run = Arc::new(SearchRun {
                        mode,
                        text: text.to_string(),
                        answer: Mutex::new(None),
                        done: Condvar::new(),
                    });
                    *slot = Some(SearchSlot::Running(run.clone()));
                    break run;
                }
            }
        };
        // Publishes the answer when dropped, also if git panics (joined searches never hang).
        let mut publish = Publish { search: self, run, answer: RunAnswer::Abandoned };
        let result = git().map(Arc::<[u32]>::from);
        publish.answer = match &result {
            Ok(hits) => RunAnswer::Hits(hits.clone()),
            Err(_) if is_set(cancel) => RunAnswer::Abandoned,
            Err(e) => RunAnswer::Failed(e.to_string()),
        };
        result
    }
}

/// Ends a `SearchRun` (see `WalkSearch::hits`).
struct Publish<'a> {
    search: &'a WalkSearch,
    run: Arc<SearchRun>,
    answer: RunAnswer,
}

impl Drop for Publish<'_> {
    fn drop(&mut self) {
        let answer = std::mem::replace(&mut self.answer, RunAnswer::Abandoned);
        {
            let mut slot = self.search.slot.lock().unwrap_or_else(PoisonError::into_inner);
            // Unless another query took the slot meanwhile: keep the hits for later searches; a failure is
            // not kept (the next search runs git again).
            if matches!(&*slot, Some(SearchSlot::Running(r)) if Arc::ptr_eq(r, &self.run)) {
                *slot = match &answer {
                    RunAnswer::Hits(hits) => Some(SearchSlot::Done {
                        mode: self.run.mode,
                        text: self.run.text.clone(),
                        hits: hits.clone(),
                    }),
                    _ => None,
                };
            }
        }
        // After the slot: a joined search that finds the run abandoned looks at the slot again.
        *self.run.answer.lock().unwrap_or_else(PoisonError::into_inner) = Some(answer);
        self.run.done.notify_all();
    }
}

fn is_set(flag: Option<&AtomicBool>) -> bool {
    flag.is_some_and(|f| f.load(Ordering::SeqCst))
}

const EMPTY_SLOT: u32 = u32::MAX;

/// oid → commit index: open addressing over `u32` slots (load <= 1/2); the keys stay in the oid pool.
struct OidIndex {
    slots: Vec<u32>,
}

impl OidIndex {
    fn build(oids: &StrPool) -> OidIndex {
        let cap = (oids.len() * 2).max(16).next_power_of_two();
        let mask = cap - 1;
        let mut slots = vec![EMPTY_SLOT; cap];
        for i in 0..oids.len() {
            let key = oids.get(i).as_bytes();
            let mut s = hash_oid(key) & mask;
            loop {
                let cur = slots[s];
                if cur == EMPTY_SLOT {
                    slots[s] = i as u32;
                    break;
                }
                // A repeated oid keeps its first row.
                if oids.get(cur as usize).as_bytes() == key {
                    break;
                }
                s = (s + 1) & mask;
            }
        }
        OidIndex { slots }
    }

    fn get(&self, oids: &StrPool, key: &[u8]) -> Option<u32> {
        let mask = self.slots.len() - 1;
        let mut s = hash_oid(key) & mask;
        loop {
            let cur = self.slots[s];
            if cur == EMPTY_SLOT {
                return None;
            }
            if oids.get(cur as usize).as_bytes() == key {
                return Some(cur);
            }
            s = (s + 1) & mask;
        }
    }
}

/// FNV-1a with a final avalanche (the slot is taken from the low bits).
fn hash_oid(key: &[u8]) -> usize {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in key {
        h = (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
    }
    h ^= h >> 33;
    h = h.wrapping_mul(0xff51_afd7_ed55_8ccd);
    h ^= h >> 33;
    h as usize
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{GraphOrder, HeadInfo, LocalBranch, RemoteBranch, Stash, Worktree};

    fn oid(n: u8) -> String {
        format!("{n:02x}").repeat(20)
    }

    fn opts() -> GraphOptions {
        GraphOptions {
            order: GraphOrder::Date,
            limit: 0,
            include_remotes: true,
            include_tags: true,
            include_stashes: true,
            hidden_refs: Vec::new(),
            only_refs: Vec::new(),
            pin_head_lane: true,
            include_head: true,
            first_parent: false,
        }
    }

    /// `git log -z` output for (oid, parents, author, subject) records, newest first.
    fn log_bytes(commits: &[(u8, &[u8], &str, &str)]) -> Vec<u8> {
        let mut out = Vec::new();
        for (i, (n, parents, author, subject)) in commits.iter().enumerate() {
            let ps: Vec<String> = parents.iter().map(|&p| oid(p)).collect();
            let email = format!("{}@example.com", author.to_lowercase());
            let time = 1_700_000_000 - i as i64 * 100;
            let rec = format!("{}\0{}\0{author}\0{email}\0{time}\0{subject}\0", oid(*n), ps.join(" "));
            out.extend_from_slice(rec.as_bytes());
        }
        out
    }

    const HISTORY: &[(u8, &[u8], &str, &str)] = &[
        (0x11, &[0x22, 0x44], "Ann", "Merge feature"),
        (0x22, &[0x33], "Bob", "Fix bug"),
        (0x33, &[0x55], "Ann", "한글 제목 Ünïcode"),
        (0x44, &[0x55], "Bob", "feature work"),
        (0x55, &[], "Ann", "root"),
    ];

    fn local(name: &str, n: u8, is_head: bool) -> LocalBranch {
        LocalBranch {
            name: name.into(),
            full_name: format!("refs/heads/{name}"),
            oid: oid(n),
            upstream: None,
            ahead: 0,
            behind: 0,
            upstream_gone: false,
            is_head,
            worktree: None,
            time: 0,
        }
    }

    fn remote(name: &str, n: u8) -> RemoteBranch {
        let (r, short) = name.split_once('/').unwrap();
        RemoteBranch {
            remote: r.into(),
            name: name.into(),
            short: short.into(),
            full_name: format!("refs/remotes/{name}"),
            oid: oid(n),
            time: 0,
        }
    }

    /// Lightweight tag on commit `n`.
    fn tag(name: &str, n: u8) -> Tag {
        Tag {
            name: name.into(),
            full_name: format!("refs/tags/{name}"),
            oid: oid(n),
            target_type: "commit".into(),
            annotated: false,
            message: None,
            time: 0,
        }
    }

    /// Tag peeling to a `target_type` object `n` that is no commit.
    fn tag_on(name: &str, n: u8, target_type: &str) -> Tag {
        Tag { target_type: target_type.into(), ..tag(name, n) }
    }

    fn stash(index: u32, n: u8, base: u8, message: &str, author: &str) -> Stash {
        Stash {
            index,
            name: format!("stash@{{{index}}}"),
            oid: oid(n),
            base: oid(base),
            index_commit: None,
            untracked_commit: None,
            message: message.into(),
            author: author.into(),
            email: format!("{}@example.com", author.to_lowercase()),
            time: 1_800_000_000 + i64::from(index),
        }
    }

    fn worktree(path: &str, head: u8, branch: Option<&str>, current: bool) -> Worktree {
        Worktree {
            path: path.into(),
            head: Some(oid(head)),
            branch: branch.map(String::from),
            detached: branch.is_none(),
            bare: false,
            locked: false,
            prunable: false,
            is_main: current,
            is_current: current,
        }
    }

    fn snapshot() -> RefsSnapshot {
        RefsSnapshot {
            head: HeadInfo { oid: Some(oid(0x11)), branch: Some("main".into()), detached: false, unborn: false },
            locals: vec![
                local("alpha", 0x11, false),
                local("feature", 0x44, false),
                local("main", 0x11, true),
                local("zeta", 0x11, false),
            ],
            remotes: vec![remote("origin/main", 0x11), remote("origin/alpha", 0x11)],
            tags: vec![tag("v2", 0x11), tag("v1", 0x33), tag_on("tree-tag", 0xee, "tree")],
            stashes: vec![
                stash(0, 0xa0, 0x33, "WIP on main: newest", "Cara"),
                stash(1, 0xa1, 0x33, "On main: older", "Dan"),
                stash(2, 0xa2, 0x99, "On gone: lost", "Eve"),
            ],
            worktrees: vec![
                worktree("/r/main", 0x11, Some("main"), true),
                worktree("/r/wt-detached", 0x22, None, false),
                worktree("/r/wt-feature", 0x44, Some("feature"), false),
                worktree("/r/wt-top", 0x11, None, false),
            ],
            ..Default::default()
        }
    }

    fn cache_for(
        snap: &RefsSnapshot,
        history: &[(u8, &[u8], &str, &str)],
        opts: &GraphOptions,
        wip: bool,
    ) -> GraphCache {
        let walk = Walk {
            tips: select_tips(snap, opts),
            shallow: Vec::new(),
            commits: Arc::new(Commits::parse(&log_bytes(history), opts.limit, opts.first_parent)),
        };
        GraphCache::assemble(false, opts, wip, 7, snap, walk, None)
    }

    fn fixture(opts: &GraphOptions, wip: bool) -> GraphCache {
        cache_for(&snapshot(), HISTORY, opts, wip)
    }

    fn info() -> RepoInfo {
        RepoInfo {
            path: "/nonexistent-git-viewer-repo".into(),
            name: "r".into(),
            git_dir: "/nonexistent-git-viewer-repo/.git".into(),
            common_dir: "/nonexistent-git-viewer-repo/.git".into(),
            is_bare: false,
            is_linked_worktree: false,
        }
    }

    fn quick(c: &GraphCache, text: &str) -> Vec<u32> {
        c.search(&info(), &SearchQuery { text: text.into(), mode: SearchMode::Quick }, None).unwrap()
    }

    fn oids(ns: &[u8]) -> Vec<String> {
        ns.iter().map(|&n| oid(n)).collect()
    }

    #[test]
    fn tips_default_and_include_flags() {
        let snap = snapshot();
        assert_eq!(select_tips(&snap, &opts()), oids(&[0x11, 0x22, 0x33, 0x44, 0x99]));
        let o = GraphOptions { include_remotes: false, include_tags: false, include_stashes: false, ..opts() };
        assert_eq!(select_tips(&snap, &o), oids(&[0x11, 0x22, 0x44]));
        // Remotes only: their oids are also locals here; tags only add v1 (the tree tag is no commit).
        let o = GraphOptions { include_remotes: false, include_stashes: false, ..opts() };
        assert_eq!(select_tips(&snap, &o), oids(&[0x11, 0x22, 0x33, 0x44]));
    }

    #[test]
    fn tips_hidden_refs() {
        let snap = snapshot();
        let base = GraphOptions { include_remotes: false, include_tags: false, include_stashes: false, ..opts() };
        // Hiding a branch also drops another worktree's HEAD that has it checked out.
        let o = GraphOptions { hidden_refs: vec!["refs/heads/feature".into()], ..base.clone() };
        assert_eq!(select_tips(&snap, &o), oids(&[0x11, 0x22]));
        // HEAD is always kept, even when every ref at it is hidden.
        let hidden = ["refs/heads/main", "refs/heads/alpha", "refs/heads/zeta", "refs/heads/feature"];
        let mut snap2 = snapshot();
        snap2.worktrees.retain(|w| w.is_current || !w.detached);
        let o = GraphOptions { hidden_refs: hidden.iter().map(|s| s.to_string()).collect(), ..base };
        assert_eq!(select_tips(&snap2, &o), oids(&[0x11]));
        // A hidden ref doesn't remove an oid that another visible ref still points to.
        let o = GraphOptions { hidden_refs: vec!["refs/tags/v2".into(), "refs/remotes/origin/main".into()], ..opts() };
        assert_eq!(select_tips(&snap, &o), oids(&[0x11, 0x22, 0x33, 0x44, 0x99]));
    }

    #[test]
    fn tips_hidden_branch_named_like_a_ref() {
        // Worktree branches come without `refs/heads/`: a branch called `refs/x` is `refs/heads/refs/x`.
        let base = GraphOptions { include_remotes: false, include_tags: false, include_stashes: false, ..opts() };
        let mut snap = snapshot();
        snap.locals.push(local("refs/x", 0x22, false));
        snap.worktrees = vec![
            worktree("/r/main", 0x11, Some("main"), true),
            worktree("/r/wt-x", 0x22, Some("refs/x"), false),
            worktree("/r/wt-remote", 0x33, Some("refs/remotes/origin/main"), false),
        ];
        assert_eq!(select_tips(&snap, &base), oids(&[0x11, 0x22, 0x33, 0x44]));
        let hide =
            |refs: &[&str]| GraphOptions { hidden_refs: refs.iter().map(|s| s.to_string()).collect(), ..base.clone() };
        assert_eq!(select_tips(&snap, &hide(&["refs/heads/refs/x"])), oids(&[0x11, 0x33, 0x44]));
        // A HEAD symref outside refs/heads keeps its full name.
        assert_eq!(select_tips(&snap, &hide(&["refs/remotes/origin/main"])), oids(&[0x11, 0x22, 0x44]));
    }

    #[test]
    fn tips_only_refs() {
        let snap = snapshot();
        let o = GraphOptions { only_refs: vec!["refs/heads/feature".into()], include_stashes: false, ..opts() };
        assert_eq!(select_tips(&snap, &o), oids(&[0x11, 0x44]));
        let o = GraphOptions { only_refs: vec!["refs/tags/v1".into()], ..opts() };
        assert_eq!(select_tips(&snap, &o), oids(&[0x11, 0x33, 0x99]));
        // Soloing a ref that no longer exists leaves HEAD.
        let o = GraphOptions { only_refs: vec!["refs/heads/gone".into()], include_stashes: false, ..opts() };
        assert_eq!(select_tips(&snap, &o), oids(&[0x11]));
        // Unborn HEAD without refs: nothing to walk.
        assert!(select_tips(&RefsSnapshot::default(), &opts()).is_empty());
    }

    #[test]
    fn tips_without_head() {
        // HEAD detached on a commit no ref points to; no other worktrees.
        let mut snap = snapshot();
        snap.head = HeadInfo { oid: Some(oid(0x55)), branch: None, detached: true, unborn: false };
        snap.worktrees.retain(|w| w.is_current);
        let base = GraphOptions { include_remotes: false, include_tags: false, include_stashes: false, ..opts() };
        assert_eq!(select_tips(&snap, &base), oids(&[0x11, 0x44, 0x55]));
        let no_head = GraphOptions { include_head: false, ..base.clone() };
        assert_eq!(select_tips(&snap, &no_head), oids(&[0x11, 0x44]));
        // Soloed (the simple view's branch): the ref alone, HEAD only when asked for.
        let only = vec!["refs/heads/feature".to_string()];
        assert_eq!(select_tips(&snap, &GraphOptions { only_refs: only.clone(), ..base }), oids(&[0x44, 0x55]));
        assert_eq!(select_tips(&snap, &GraphOptions { only_refs: only, ..no_head.clone() }), oids(&[0x44]));
        // A branch that is gone leaves nothing to walk.
        let gone = GraphOptions { only_refs: vec!["refs/heads/gone".into()], ..no_head };
        assert!(select_tips(&snap, &gone).is_empty());
    }

    /// `git log --first-parent` output along HISTORY's line: the merge still lists both parents (`%P`),
    /// but the commit it merged (0x44) is not walked.
    const FIRST_PARENT: &[(u8, &[u8], &str, &str)] = &[
        (0x11, &[0x22, 0x44], "Ann", "Merge pull request #7 from acme/feature"),
        (0x22, &[0x33], "Bob", "Fix bug"),
        (0x33, &[0x55], "Ann", "한글 제목 Ünïcode"),
        (0x55, &[], "Ann", "root"),
    ];

    #[test]
    fn first_parent_walk_keeps_first_parents() {
        let o = GraphOptions { first_parent: true, include_stashes: false, pin_head_lane: false, ..opts() };
        let c = cache_for(&snapshot(), FIRST_PARENT, &o, false);
        let rows = c.rows(0, 100);
        assert_eq!(rows.iter().map(|r| r.oid.clone()).collect::<Vec<_>>(), oids(&[0x11, 0x22, 0x33, 0x55]));
        assert_eq!(rows[0].parents, [oid(0x22)]);
        // The merged commit is no parent cut off by the walk: nothing is excluded from searches, and
        // the line is one lane.
        assert_eq!(c.walk.commits.missing.len(), 0);
        assert!(c.search_revs().iter().all(|r| !r.starts_with('^')));
        assert_eq!(c.summary().max_lanes, 1);
        assert!(rows.iter().all(|r| r.lane == 0));
        // Read as a full walk's output, the merged commit would be a parent outside the walk.
        let full = cache_for(&snapshot(), FIRST_PARENT, &GraphOptions { first_parent: false, ..o }, false);
        assert_eq!(full.rows(0, 1)[0].parents, oids(&[0x22, 0x44]));
        assert_eq!(full.walk.commits.missing.len(), 1);
        assert_eq!(full.summary().max_lanes, 2);
    }

    #[test]
    fn pull_request_titles_of_a_first_parent_walk() {
        let o = GraphOptions { first_parent: true, include_stashes: false, ..opts() };
        let mut commits = Commits::parse(&log_bytes(FIRST_PARENT), 0, true);
        // `log::read_merge_bodies` output: 0x11 has a title; a commit outside the walk and one whose
        // subject is no merge line get none.
        let bodies = format!(
            "{}\0Add the feature\n\0{}\0Other\n\0{}\0Not a title\n\0",
            oid(0x11),
            oid(0x44),
            oid(0x22)
        );
        commits.set_titles(bodies.as_bytes());
        assert_eq!(commits.titles, [(0, "Add the feature".to_string())]);
        let walk = Walk { tips: select_tips(&snapshot(), &o), shallow: Vec::new(), commits: Arc::new(commits) };
        let c = GraphCache::assemble(false, &o, true, 3, &snapshot(), walk, None);
        let titles: Vec<Option<String>> = c.rows(0, 100).into_iter().map(|r| r.title).collect();
        assert_eq!(titles, [None, Some("Add the feature".to_string()), None, None, None]);
        // Quick search finds the merge by its title as well as by its subject (row 1, under the WIP row).
        assert_eq!(quick(&c, "THE FEATURE"), [1]);
        assert_eq!(quick(&c, "acme/feature"), [1]);
        // Without bodies, no titles (a full walk never reads them).
        assert!(Commits::parse(&log_bytes(FIRST_PARENT), 0, true).titles.is_empty());
    }

    #[test]
    fn non_commit_tags_are_neither_walked_nor_labelled() {
        // A tree or blob never shares a commit's oid; borrowing loaded commits' oids here shows that the
        // target type alone keeps such tags out, soloed or not.
        let mut snap = snapshot();
        snap.locals.retain(|b| b.is_head);
        snap.worktrees.retain(|w| w.is_current);
        snap.tags = vec![tag("v1", 0x33), tag_on("tree-tag", 0x22, "tree"), tag_on("blob-tag", 0x44, "blob")];
        let tag_labels = |o: &GraphOptions| -> Vec<(usize, String)> {
            let rows = cache_for(&snap, HISTORY, o, false).rows(0, 100);
            rows.iter()
                .enumerate()
                .flat_map(|(i, r)| {
                    r.refs.iter().filter(|l| l.kind == RefLabelKind::Tag).map(move |l| (i, l.name.clone()))
                })
                .collect()
        };
        let o = GraphOptions { include_remotes: false, include_stashes: false, ..opts() };
        assert_eq!(select_tips(&snap, &o), oids(&[0x11, 0x33]));
        assert_eq!(tag_labels(&o), [(2, "v1".to_string())]);
        // Soloed: only HEAD is walked, and the tag still gets no label (v1's kind is included).
        let solo = GraphOptions { only_refs: vec!["refs/tags/tree-tag".into()], ..o };
        assert_eq!(select_tips(&snap, &solo), oids(&[0x11]));
        assert_eq!(tag_labels(&solo), [(2, "v1".to_string())]);
    }

    #[test]
    fn stash_rows_spliced_above_base_under_wip() {
        let c = fixture(&opts(), true);
        let rows = c.rows(0, 100);
        let kinds: Vec<RowKind> = rows.iter().map(|r| r.kind).collect();
        use RowKind::*;
        assert_eq!(kinds, [Wip, Commit, Commit, Stash, Stash, Commit, Commit, Commit]);
        let got: Vec<&str> = rows.iter().map(|r| r.oid.as_str()).collect();
        let want = ["".to_string(), oid(0x11), oid(0x22), oid(0xa0), oid(0xa1), oid(0x33), oid(0x44), oid(0x55)];
        assert_eq!(got, want);
        assert_eq!(rows[0].parents, [oid(0x11)]);
        assert_eq!((rows[0].subject.as_str(), rows[0].author.as_str(), rows[0].time), ("", "", 0));
        assert_eq!(rows[1].parents, [oid(0x22), oid(0x44)]);
        assert_eq!(rows[3].parents, [oid(0x33)]);
        assert_eq!(rows[3].subject, "WIP on main: newest");
        assert_eq!((rows[3].author.as_str(), rows[3].email.as_str()), ("Cara", "cara@example.com"));
        assert_eq!(rows[3].time, 1_800_000_000);
        assert_eq!(rows[4].subject, "On main: older");
        assert_eq!((rows[4].author.as_str(), rows[4].email.as_str()), ("Dan", "dan@example.com"));
        assert_eq!(rows[5].subject, "한글 제목 Ünïcode");
        assert_eq!((rows[5].author.as_str(), rows[5].email.as_str()), ("Ann", "ann@example.com"));
        assert!(rows[7].parents.is_empty());

        let s = c.summary();
        assert_eq!((s.generation, s.total, s.head_row, s.wip_row, s.truncated), (7, 8, Some(1), Some(0), false));

        let nodes = c.layout_nodes();
        let parents: Vec<Vec<u32>> = nodes.iter().map(|n| n.parents.clone()).collect();
        assert_eq!(parents, [vec![1], vec![2, 6], vec![5], vec![5], vec![5], vec![7], vec![7], vec![]]);
        let dashed: Vec<bool> = nodes.iter().map(|n| n.dashed).collect();
        assert_eq!(dashed, [true, false, false, true, true, false, false, false]);
    }

    #[test]
    fn rows_without_wip_or_stashes() {
        let c = fixture(&opts(), false);
        let s = c.summary();
        assert_eq!((s.total, s.head_row, s.wip_row), (7, Some(0), None));
        assert_eq!(c.rows(0, 7)[2].kind, RowKind::Stash);

        let c = fixture(&GraphOptions { include_stashes: false, ..opts() }, true);
        let kinds: Vec<RowKind> = c.rows(0, 100).iter().map(|r| r.kind).collect();
        assert_eq!(
            kinds,
            [RowKind::Wip, RowKind::Commit, RowKind::Commit, RowKind::Commit, RowKind::Commit, RowKind::Commit]
        );

        // A bare repo never gets a WIP row.
        let snap = snapshot();
        let walk =
            Walk { tips: Vec::new(), shallow: Vec::new(), commits: Arc::new(Commits::parse(&log_bytes(HISTORY), 0, false)) };
        let c = GraphCache::assemble(true, &opts(), true, 1, &snap, walk, None);
        assert_eq!(c.summary().wip_row, None);
        assert_eq!(c.find(""), None);
    }

    #[test]
    fn wip_on_unborn_head() {
        let c = cache_for(&RefsSnapshot::default(), &[], &opts(), true);
        let rows = c.rows(0, 10);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].kind, RowKind::Wip);
        assert!(rows[0].parents.is_empty());
        let s = c.summary();
        assert_eq!((s.total, s.head_row, s.wip_row), (1, None, Some(0)));
        assert_eq!(c.layout_nodes().len(), 1);
        assert!(c.layout_nodes()[0].parents.is_empty());
        assert!(cache_for(&RefsSnapshot::default(), &[], &opts(), false).rows(0, 10).is_empty());
    }

    #[test]
    fn wip_row_reaches_a_head_below_newer_commits() {
        use crate::model::{EDGE_DASHED as D, EDGE_FROM_NODE as F, EDGE_TO_NODE as T};
        // The checked-out branch is behind its remote (not pulled yet, or reset back) and the tree has
        // changes: the WIP row tops the graph in lane 0 and its dashed line runs down past the newer
        // commits, which take lane 1 and bend into HEAD's node.
        let history: &[(u8, &[u8], &str, &str)] = &[
            (0x11, &[0x22], "Ann", "newest"),
            (0x22, &[0x33], "Ann", "newer"),
            (0x33, &[0x44], "Ann", "head"),
            (0x44, &[], "Ann", "root"),
        ];
        let mut snap = RefsSnapshot {
            head: HeadInfo { oid: Some(oid(0x33)), branch: Some("main".into()), detached: false, unborn: false },
            locals: vec![local("main", 0x33, true)],
            remotes: vec![remote("origin/main", 0x11)],
            ..Default::default()
        };
        let lanes = |c: &GraphCache| c.rows(0, 10).iter().map(|r| r.lane).collect::<Vec<u16>>();
        for pin_head_lane in [true, false] {
            let c = cache_for(&snap, history, &GraphOptions { pin_head_lane, ..opts() }, true);
            let s = c.summary();
            assert_eq!((s.total, s.wip_row, s.head_row, s.max_lanes), (5, Some(0), Some(3), 2));
            let rows = c.rows(0, 10);
            assert_eq!(rows[0].parents, [oid(0x33)]);
            assert_eq!(lanes(&c), [0, 1, 1, 0, 0]);
            assert_eq!(rows[0].edges, [Edge(0, 0, 0, F | D)]);
            assert_eq!(rows[1].edges, [Edge(1, 1, 1, F | T), Edge(0, 0, 0, D)]);
            assert_eq!(rows[2].edges, [Edge(1, 0, 1, F | T), Edge(0, 0, 0, D | T)]);
            assert_eq!(rows[3].edges, [Edge(0, 0, 0, F | T)]);
            assert_eq!(labels(&rows, 1), [(RefLabelKind::Remote, "origin/main", false)]);
            assert_eq!(labels(&rows, 3), [(RefLabelKind::Local, "main", true)]);
        }
        // A clean tree has no WIP row: the pinned HEAD keeps lane 0 under the newer commits; unpinned,
        // the history is one line.
        let clean = cache_for(&snap, history, &opts(), false);
        assert_eq!((clean.summary().wip_row, clean.summary().head_row), (None, Some(2)));
        assert_eq!(lanes(&clean), [1, 1, 0, 0]);
        assert_eq!(lanes(&cache_for(&snap, history, &GraphOptions { pin_head_lane: false, ..opts() }, false)), [0; 4]);
        // The same with HEAD detached on an older commit (a checked-out commit): `main` is now among
        // the commits the line passes.
        snap.head = HeadInfo { oid: Some(oid(0x44)), branch: None, detached: true, unborn: false };
        snap.locals = vec![local("main", 0x33, false)];
        let c = cache_for(&snap, history, &opts(), true);
        let rows = c.rows(0, 10);
        assert_eq!((c.summary().wip_row, c.summary().head_row), (Some(0), Some(4)));
        assert_eq!(lanes(&c), [0, 1, 1, 1, 0]);
        assert_eq!(rows[3].edges, [Edge(1, 0, 1, F | T), Edge(0, 0, 0, D | T)]);
        assert_eq!(labels(&rows, 3), [(RefLabelKind::Local, "main", false)]);
        assert_eq!(labels(&rows, 4), [(RefLabelKind::Head, "HEAD", true)]);
    }

    #[test]
    fn find_and_clamped_ranges() {
        let c = fixture(&opts(), true);
        assert_eq!(c.find(""), Some(0));
        assert_eq!(c.find(&oid(0x11)), Some(1));
        assert_eq!(c.find(&oid(0x33)), Some(5));
        assert_eq!(c.find(&oid(0xa1)), Some(4));
        assert_eq!(c.find(&oid(0xa2)), None);
        assert_eq!(c.find(&oid(0x99)), None);
        assert_eq!(c.find("1111"), None);

        let mid: Vec<String> = c.rows(3, 5).into_iter().map(|r| r.oid).collect();
        assert_eq!(mid, [oid(0xa0), oid(0xa1)]);
        let tail: Vec<String> = c.rows(5, 100).into_iter().map(|r| r.oid).collect();
        assert_eq!(tail, oids(&[0x33, 0x44, 0x55]));
        assert!(c.rows(8, 9).is_empty());
        assert!(c.rows(100, 200).is_empty());
        assert!(c.rows(5, 2).is_empty());
        // Labels stay aligned when a range starts mid-way.
        assert_eq!(c.rows(4, 6)[1].refs.iter().map(|l| l.name.as_str()).collect::<Vec<_>>(), ["v1"]);
    }

    fn labels(rows: &[GraphRow], row: usize) -> Vec<(RefLabelKind, &str, bool)> {
        rows[row].refs.iter().map(|l| (l.kind, l.name.as_str(), l.current)).collect()
    }

    #[test]
    fn decoration_order() {
        use RefLabelKind::*;
        let c = fixture(&opts(), true);
        let rows = c.rows(0, 100);
        assert!(rows[0].refs.is_empty());
        assert_eq!(
            labels(&rows, 1),
            [
                (Local, "main", true),
                (Local, "alpha", false),
                (Local, "zeta", false),
                (Remote, "origin/alpha", false),
                (Remote, "origin/main", false),
                (Tag, "v2", false),
                (Worktree, "wt-top", false),
            ]
        );
        assert_eq!(labels(&rows, 2), [(Worktree, "wt-detached", false)]);
        assert_eq!(rows[2].refs[0].full_name, "/r/wt-detached");
        assert_eq!(labels(&rows, 3), [(Stash, "stash@{0}", false)]);
        assert_eq!(labels(&rows, 4), [(Stash, "stash@{1}", false)]);
        assert_eq!(labels(&rows, 5), [(Tag, "v1", false)]);
        assert_eq!(labels(&rows, 6), [(Local, "feature", false)]);
        assert_eq!(rows[6].refs[0].full_name, "refs/heads/feature");
        assert!(rows[7].refs.is_empty());
    }

    #[test]
    fn decoration_detached_head() {
        use RefLabelKind::*;
        let mut snap = snapshot();
        snap.head = HeadInfo { oid: Some(oid(0x22)), branch: None, detached: true, unborn: false };
        for b in &mut snap.locals {
            b.is_head = false;
        }
        let c = cache_for(&snap, HISTORY, &opts(), false);
        let rows = c.rows(0, 100);
        assert_eq!(labels(&rows, 1), [(Head, "HEAD", true), (Worktree, "wt-detached", false)]);
        assert_eq!(rows[1].refs[0].full_name, "HEAD");
        assert_eq!(labels(&rows, 0)[0], (Local, "alpha", false));
        assert_eq!(c.summary().head_row, Some(1));
    }

    #[test]
    fn decoration_follows_filters() {
        use RefLabelKind::*;
        let o = GraphOptions {
            include_tags: false,
            hidden_refs: vec!["refs/heads/zeta".into(), "refs/remotes/origin/main".into()],
            ..opts()
        };
        let rows = fixture(&o, false).rows(0, 100);
        assert_eq!(
            labels(&rows, 0),
            [
                (Local, "main", true),
                (Local, "alpha", false),
                (Remote, "origin/alpha", false),
                (Worktree, "wt-top", false)
            ]
        );
        assert!(rows[4].refs.is_empty());
        // A soloed ref is labelled even when its kind is excluded.
        let o = GraphOptions { include_tags: false, only_refs: vec!["refs/tags/v1".into()], ..opts() };
        let rows = fixture(&o, false).rows(0, 100);
        assert_eq!(labels(&rows, 4), [(Tag, "v1", false)]);
    }

    #[test]
    fn quick_search_matches() {
        let c = fixture(&opts(), true);
        assert_eq!(quick(&c, "ÜNÏ"), [5]);
        assert_eq!(quick(&c, "한글"), [5]);
        assert_eq!(quick(&c, "FEATURE"), [1, 6]);
        assert_eq!(quick(&c, "bob"), [2, 6]);
        assert_eq!(quick(&c, "BOB@EXAMPLE"), [2, 6]);
        assert_eq!(quick(&c, "4444"), [6]);
        assert_eq!(quick(&c, &oid(0x44)), [6]);
        assert!(quick(&c, "444").is_empty());
        assert_eq!(quick(&c, "stash@{1}"), [4]);
        assert_eq!(quick(&c, "newest"), [3]);
        assert_eq!(quick(&c, "cara"), [3]);
        assert_eq!(quick(&c, "DAN@example"), [4]);
        assert_eq!(quick(&c, "A0A0"), [3]);
        assert_eq!(quick(&c, "origin/al"), [1]);
        assert_eq!(quick(&c, "wip"), [3]);
        assert_eq!(quick(&c, "wt-"), [1, 2]);
        assert!(quick(&c, "  ").is_empty());
        assert!(quick(&c, "nothing like this").is_empty());
        let all = quick(&c, "a");
        assert!(all.windows(2).all(|w| w[0] < w[1]));
        assert!(all.contains(&1) && all.contains(&7) && !all.contains(&0));
    }

    #[test]
    fn git_search_reuses_the_walk_result() {
        let search = |c: &GraphCache, mode: SearchMode, text: &str| {
            c.search(&info(), &SearchQuery { text: text.into(), mode }, None)
        };
        let c = fixture(&opts(), true);
        // Commits 0 (0x11) and 2 (0x33), as a previous run over this walk found them.
        *c.walk.commits.search.slot.lock().unwrap() =
            Some(SearchSlot::Done { mode: SearchMode::Content, text: "needle".into(), hits: Arc::from(vec![0u32, 2]) });
        assert_eq!(search(&c, SearchMode::Content, " needle ").unwrap(), [1, 5]);
        // A generation that reuses the walk maps the same commits to its own rows (no WIP row here).
        let walk = Walk { tips: c.walk.tips.clone(), shallow: Vec::new(), commits: c.walk.commits.clone() };
        let next = GraphCache::assemble(false, &opts(), false, 8, &snapshot(), walk, Some(&c));
        assert_eq!(search(&next, SearchMode::Content, "needle").unwrap(), [0, 4]);
        // Same walk, same key; a walk read again gets another one.
        let q = SearchQuery { text: " needle ".into(), mode: SearchMode::Content };
        assert_eq!(c.search_key(&q), next.search_key(&q));
        assert_eq!(c.search_key(&q).text, "needle");
        assert_ne!(c.search_key(&q), fixture(&opts(), true).search_key(&q));
        // Any other query runs git (which fails here: the repo doesn't exist).
        assert!(search(&c, SearchMode::Message, "needle").is_err());
        assert!(search(&c, SearchMode::Content, "needles").is_err());
    }

    /// A search thread and the sender that releases its `git`.
    type HeldSearch = (std::thread::JoinHandle<AppResult<Arc<[u32]>>>, std::sync::mpsc::Sender<()>);

    /// Runs `WalkSearch::hits` on a thread with a `git` that reports it started, then waits for a
    /// release before answering `answer`.
    fn held_search(s: &Arc<WalkSearch>, cancel: Arc<AtomicBool>, answer: AppResult<Vec<u32>>) -> HeldSearch {
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
        let s = s.clone();
        let owner = std::thread::spawn(move || {
            s.hits(SearchMode::Content, "x", Some(&cancel), move || {
                started_tx.send(()).unwrap();
                release_rx.recv().unwrap();
                answer
            })
        });
        started_rx.recv().unwrap();
        (owner, release_tx)
    }

    #[test]
    fn walk_search_keeps_its_answer() {
        let s = WalkSearch::default();
        let first = s.hits(SearchMode::Content, "x", None, || Ok(vec![1, 4])).unwrap();
        assert_eq!(*first, [1, 4]);
        let again = s.hits(SearchMode::Content, "x", None, || panic!("git ran again")).unwrap();
        assert!(Arc::ptr_eq(&first, &again));
        // Another query runs git; a failed run is not kept.
        assert!(s.hits(SearchMode::Message, "x", None, || Err(AppError::msg("boom"))).is_err());
        assert_eq!(*s.hits(SearchMode::Message, "x", None, || Ok(vec![2])).unwrap(), [2]);
    }

    #[test]
    fn identical_searches_share_one_git_run() {
        let s = Arc::new(WalkSearch::default());
        let (owner, release) = held_search(&s, Arc::new(AtomicBool::new(false)), Ok(vec![3]));
        // A search whose own flag is set leaves without waiting for the run.
        let cancelled = AtomicBool::new(true);
        assert!(s.hits(SearchMode::Content, "x", Some(&cancelled), || panic!("second git run")).is_err());
        let joiner = {
            let s = s.clone();
            std::thread::spawn(move || s.hits(SearchMode::Content, "x", None, || panic!("second git run")))
        };
        std::thread::sleep(Duration::from_millis(30));
        release.send(()).unwrap();
        let hits = owner.join().unwrap().unwrap();
        assert_eq!(*hits, [3]);
        assert!(Arc::ptr_eq(&hits, &joiner.join().unwrap().unwrap()));
    }

    #[test]
    fn a_cancelled_search_leaves_the_slot_to_the_running_one() {
        let s = Arc::new(WalkSearch::default());
        let (owner, release) = held_search(&s, Arc::new(AtomicBool::new(false)), Ok(vec![3]));
        // Superseded by the running query before it got here: no git run, and the slot stays with it.
        let cancelled = AtomicBool::new(true);
        assert!(s.hits(SearchMode::Message, "y", Some(&cancelled), || panic!("cancelled search ran git")).is_err());
        release.send(()).unwrap();
        assert_eq!(*owner.join().unwrap().unwrap(), [3]);
        assert_eq!(*s.hits(SearchMode::Content, "x", None, || panic!("git ran again")).unwrap(), [3]);
    }

    #[test]
    fn a_cancelled_run_is_taken_over_by_a_live_search() {
        let s = Arc::new(WalkSearch::default());
        let flag = Arc::new(AtomicBool::new(false));
        let (owner, release) = held_search(&s, flag.clone(), Err(AppError::msg("cancelled")));
        // Joins the run (or finds the slot free once it ended): either way it gets its own answer.
        let joiner = {
            let s = s.clone();
            std::thread::spawn(move || s.hits(SearchMode::Content, "x", Some(&AtomicBool::new(false)), || Ok(vec![5])))
        };
        std::thread::sleep(Duration::from_millis(30));
        flag.store(true, Ordering::SeqCst);
        release.send(()).unwrap();
        assert!(owner.join().unwrap().is_err());
        assert_eq!(*joiner.join().unwrap().unwrap(), [5]);
        assert_eq!(*s.hits(SearchMode::Content, "x", None, || panic!("git ran again")).unwrap(), [5]);
    }

    #[test]
    fn same_graph_compares_what_is_served() {
        let c = fixture(&opts(), true);
        let with = |o: &GraphOptions, wip: bool, snap: &RefsSnapshot| {
            let walk = Walk { tips: c.walk.tips.clone(), shallow: Vec::new(), commits: c.walk.commits.clone() };
            GraphCache::assemble(false, o, wip, 8, snap, walk, Some(&c))
        };
        assert!(c.same_graph(&with(&opts(), true, &snapshot())));
        // Another WIP row, pinned lane, label set or stash row.
        assert!(!c.same_graph(&with(&opts(), false, &snapshot())));
        assert!(!c.same_graph(&with(&GraphOptions { pin_head_lane: false, ..opts() }, true, &snapshot())));
        assert!(!c.same_graph(&with(&GraphOptions { include_tags: false, ..opts() }, true, &snapshot())));
        let mut snap = snapshot();
        snap.stashes[1].message = "On main: renamed".into();
        assert!(!c.same_graph(&with(&opts(), true, &snap)));
        // Without a WIP row nor HEAD row there is nothing to pin.
        let unpinned = |pin_head_lane: bool| {
            let mut snap = snapshot();
            snap.head.oid = None;
            with(&GraphOptions { pin_head_lane, ..opts() }, false, &snap)
        };
        assert!(unpinned(true).same_graph(&unpinned(false)));
        // The same commits read again are another walk.
        assert!(!c.same_graph(&fixture(&opts(), true)));
    }

    /// Field-by-field equality of two layouts (`Layout` has no `PartialEq`).
    fn same_layout(a: &Layout, b: &Layout) -> bool {
        a.lanes == b.lanes
            && a.colors == b.colors
            && a.edges == b.edges
            && a.offsets == b.offsets
            && a.max_lanes == b.max_lanes
    }

    #[test]
    fn layout_is_shared_while_rows_and_pin_stay() {
        // A generation over `prev`'s walk, as `build` makes it when the tips stay.
        let next = |prev: &GraphCache, o: &GraphOptions, wip: bool, snap: &RefsSnapshot| {
            let walk = Walk { tips: prev.walk.tips.clone(), shallow: Vec::new(), commits: prev.walk.commits.clone() };
            GraphCache::assemble(false, o, wip, prev.generation + 1, snap, walk, Some(prev))
        };
        let shares = |prev: &GraphCache, n: &GraphCache| {
            // Shared or not, the layout is what a layout pass over the new rows computes.
            assert!(same_layout(&n.layout, &graph::layout(&n.layout_nodes(), n.pin())));
            Arc::ptr_eq(&prev.layout, &n.layout)
        };
        let c = fixture(&opts(), true);
        // Nothing changed, or only labels (a ref filter, a reworded stash): no layout pass.
        assert!(shares(&c, &next(&c, &opts(), true, &snapshot())));
        assert!(shares(&c, &next(&c, &GraphOptions { include_tags: false, ..opts() }, true, &snapshot())));
        let mut reworded = snapshot();
        reworded.stashes[0].message = "WIP on main: reworded".into();
        assert!(shares(&c, &next(&c, &opts(), true, &reworded)));
        // The WIP row, the pin, the stash rows or the HEAD under the WIP row changed: laid out again.
        assert!(!shares(&c, &next(&c, &opts(), false, &snapshot())));
        assert!(!shares(&c, &next(&c, &GraphOptions { pin_head_lane: false, ..opts() }, true, &snapshot())));
        assert!(!shares(&c, &next(&c, &GraphOptions { include_stashes: false, ..opts() }, true, &snapshot())));
        let mut moved = snapshot();
        moved.head.oid = Some(oid(0x44));
        assert!(!shares(&c, &next(&c, &opts(), true, &moved)));
        // Without a WIP row, HEAD moving to another loaded tip moves the pinned row (unless none is).
        let clean = fixture(&opts(), false);
        assert!(!shares(&clean, &next(&clean, &opts(), false, &moved)));
        let unpinned = GraphOptions { pin_head_lane: false, ..opts() };
        let clean = fixture(&unpinned, false);
        assert!(shares(&clean, &next(&clean, &unpinned, false, &moved)));
        // The same commits read again are another walk; without a previous generation nothing is shared.
        let reread = fixture(&opts(), true).walk.commits.clone();
        let walk = Walk { tips: c.walk.tips.clone(), shallow: Vec::new(), commits: reread };
        assert!(!shares(&c, &GraphCache::assemble(false, &opts(), true, 9, &snapshot(), walk, Some(&c))));
        let walk = Walk { tips: c.walk.tips.clone(), shallow: Vec::new(), commits: c.walk.commits.clone() };
        assert!(!shares(&c, &GraphCache::assemble(false, &opts(), true, 9, &snapshot(), walk, None)));
    }

    #[test]
    fn a_reload_that_changes_nothing_reuses_the_walk_and_the_layout() {
        // A window focus: the same refs, options and WIP flag over the installed generation (no git runs:
        // the repo does not exist).
        let c = fixture(&opts(), true);
        let next = GraphCache::build(&info(), &opts(), true, 8, Some(&c), Some(Arc::new(snapshot())), None).unwrap();
        assert!(Arc::ptr_eq(&c.walk.commits, &next.walk.commits));
        assert!(Arc::ptr_eq(&c.layout, &next.layout));
        assert!(c.same_graph(&next));
        // A new tag on a commit that is already a tip: same walk and layout, another graph (its labels).
        let mut tagged = snapshot();
        tagged.tags.push(tag("v3", 0x22));
        let next = GraphCache::build(&info(), &opts(), true, 9, Some(&c), Some(Arc::new(tagged)), None).unwrap();
        assert!(Arc::ptr_eq(&c.layout, &next.layout));
        assert!(!c.same_graph(&next));
        // The WIP row went away (the tree became clean): same walk, new layout.
        let next = GraphCache::build(&info(), &opts(), false, 10, Some(&c), Some(Arc::new(snapshot())), None).unwrap();
        assert!(Arc::ptr_eq(&c.walk.commits, &next.walk.commits));
        assert!(!Arc::ptr_eq(&c.layout, &next.layout));
    }

    #[test]
    fn missing_parents_and_truncation() {
        let history: &[(u8, &[u8], &str, &str)] =
            &[(0x11, &[0x22, 0x88], "Ann", "m"), (0x22, &[0x88, 0x99], "Ann", "m2")];
        let mut snap = RefsSnapshot::default();
        snap.head.oid = Some(oid(0x77));
        snap.locals = vec![local("main", 0x11, false)];
        let o = GraphOptions { limit: 2, ..opts() };
        let c = cache_for(&snap, history, &o, true);
        assert!(c.summary().truncated);
        // rows = 3 (WIP + 2): missing 0x88 → 3, 0x99 → 4, the WIP's unloaded HEAD → 5.
        let parents: Vec<Vec<u32>> = c.layout_nodes().iter().map(|n| n.parents.clone()).collect();
        assert_eq!(parents, [vec![5], vec![2, 3], vec![3, 4]]);
        assert_eq!(c.rows(1, 2)[0].parents, oids(&[0x22, 0x88]));
        assert_eq!(c.rows(0, 1)[0].parents, oids(&[0x77]));
        // The unloaded HEAD tip is dropped: nothing loaded is reachable from it.
        assert_eq!(c.walk.tips, oids(&[0x11, 0x77]));
        assert_eq!(c.search_revs(), [oid(0x11), format!("^{}", oid(0x88)), format!("^{}", oid(0x99))]);
        // A missing HEAD shared with a commit's missing parent gets the same id.
        snap.head.oid = Some(oid(0x88));
        let c = cache_for(&snap, history, &o, true);
        assert_eq!(c.layout_nodes()[0].parents, [3]);
        assert_eq!(c.summary().head_row, None);

        let buf = log_bytes(history);
        assert!(!Commits::parse(&buf, 0, false).truncated);
        assert!(!Commits::parse(&buf, 3, false).truncated);
        assert!(Commits::parse(&buf, 1, false).truncated);
    }

    #[test]
    fn parent_above_child_is_drawn_as_missing() {
        let history: &[(u8, &[u8], &str, &str)] = &[(0x11, &[0x22], "Ann", "a"), (0x22, &[0x11], "Ann", "b")];
        let c = cache_for(&RefsSnapshot::default(), history, &opts(), false);
        let parents: Vec<Vec<u32>> = c.layout_nodes().iter().map(|n| n.parents.clone()).collect();
        assert_eq!(parents, [vec![1], vec![3]]);
        assert!(parents.iter().enumerate().all(|(i, ps)| ps.iter().all(|&p| p as usize > i)));
    }

    #[test]
    fn sent_edges_drop_far_lanes() {
        let layout = Layout {
            lanes: vec![0, 0],
            colors: vec![0, 0],
            edges: vec![
                Edge(0, 0, 0, 0),
                Edge(63, 64, 1, 0),
                Edge(64, 63, 2, 0),
                Edge(64, 65, 3, 0),
                Edge(70, 70, 4, 0),
                Edge(1, 1, 5, 0),
            ],
            offsets: vec![0, 5, 6],
            max_lanes: 71,
        };
        assert_eq!(sent_edges(&layout, 0), [Edge(0, 0, 0, 0), Edge(63, 64, 1, 0), Edge(64, 63, 2, 0)]);
        assert_eq!(sent_edges(&layout, 1), [Edge(1, 1, 5, 0)]);
        assert!(sent_edges(&layout, 2).is_empty());
        assert!(sent_edges(&Layout::default(), 0).is_empty());
    }

    #[test]
    fn walk_signature() {
        let c = fixture(&opts(), true);
        let tips = select_tips(&snapshot(), &opts());
        assert!(c.walk.same_as(&tips, &[], &c.opts, &opts()));
        // Pin, WIP and filter changes that keep the tips don't need a new walk (HEAD is one of them).
        let o = GraphOptions { pin_head_lane: false, include_remotes: false, include_head: false, ..opts() };
        assert!(c.walk.same_as(&tips, &[], &c.opts, &o));
        // A first-parent walk reads other commits from the same tips (and other parents of them).
        assert!(!c.walk.same_as(&tips, &[], &c.opts, &GraphOptions { first_parent: true, ..opts() }));
        assert!(!c.walk.same_as(&tips, b"x", &c.opts, &opts()));
        assert!(!c.walk.same_as(&tips[1..], &[], &c.opts, &opts()));
        assert!(!c.walk.same_as(&tips, &[], &c.opts, &GraphOptions { order: GraphOrder::Topo, ..opts() }));
        assert!(!c.walk.same_as(&tips, &[], &c.opts, &GraphOptions { limit: 10, ..opts() }));
    }

    #[test]
    fn oid_index_lookup() {
        let mut pool = StrPool::default();
        let keys: Vec<String> =
            (0..2000u32).map(|i| format!("{:040x}", u128::from(i).wrapping_mul(0x9e37_79b9_7f4a_7c15))).collect();
        for k in &keys {
            pool.push(k.as_bytes());
        }
        pool.push(keys[5].as_bytes());
        let index = OidIndex::build(&pool);
        for (i, k) in keys.iter().enumerate() {
            assert_eq!(index.get(&pool, k.as_bytes()), Some(i as u32));
        }
        assert_eq!(index.get(&pool, b"ffff"), None);
        assert_eq!(index.get(&pool, b""), None);
        let empty = OidIndex::build(&StrPool::default());
        assert_eq!(empty.get(&StrPool::default(), b"abcd"), None);
    }

    #[test]
    fn interns_authors() {
        let c = Commits::parse(&log_bytes(HISTORY), 0, false);
        assert_eq!(c.len(), 5);
        assert_eq!(c.names.len(), 2);
        assert_eq!(c.emails.get(1), "bob@example.com");
        assert_eq!(c.missing.len(), 0);
    }
}
