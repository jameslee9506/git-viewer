//! Branches, remotes, tags, stashes, worktrees, submodules; ahead/behind of the local branches.

use crate::error::{AppError, AppResult};
use crate::git::cmd::{lossy, split_records, Git};
use crate::git::repo;
use crate::model::{
    BranchTrack, HeadInfo, LocalBranch, RefsSnapshot, Remote, RemoteBranch, RepoInfo, Stash, Submodule, Tag,
    Worktree,
};
use memchr::{memchr, memmem};
use std::cmp::Ordering;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::thread::ScopedJoinHandle;

/// 11 NUL-terminated fields per ref; git ends every record with `\n`.
/// `%(upstream)` is shortened here: `:short` yields `remotes/origin/x` when a local `origin/x` exists.
/// No `%(upstream:track)`: it walks the history between every branch and its upstream (seconds for
/// branches far behind a busy default branch), and every refresh and graph load waits for this
/// snapshot. `track` counts ahead/behind on its own.
const REF_FORMAT: &str = "--format=%(refname)%00%(objecttype)%00%(objectname)%00%(*objecttype)%00%(*objectname)%00%(HEAD)%00%(symref)%00%(upstream)%00%(worktreepath)%00%(creatordate:unix)%00%(contents:subject)%00";
const REF_FIELDS: usize = 11;
/// What the snapshot lists. Whether an upstream outside these (custom fetch refspec) is gone takes a
/// lookup of its own.
const REF_NAMESPACES: [&str; 3] = ["refs/heads", "refs/remotes", "refs/tags"];
/// 7 NUL-terminated fields per stash. Never add `--date`: it turns `%gd` into `stash@{<date>}`.
/// `%aN`/`%aE` (mailmap applied) match the graph's commit authors, so stash rows need no extra lookup.
const STASH_FORMAT: &str = "--format=%H%x00%P%x00%gd%x00%at%x00%aN%x00%aE%x00%gs";
/// What `git stash list -z <STASH_FORMAT>` runs, minus the stash builtin's work-tree requirement (it
/// exits 128 in a bare repo, whose linked worktrees share `refs/stash`) and its extra process.
/// `--ignore-missing`: no stash → empty output, exit 0.
const STASH_ARGS: &[&str] = &["log", "-g", "--first-parent", "-z", STASH_FORMAT, "--ignore-missing", "refs/stash", "--"];
const STASH_FIELDS: usize = 7;

/// Full snapshot of all refs for the sidebar and graph decorations.
pub fn snapshot(info: &RepoInfo) -> AppResult<RefsSnapshot> {
    let cwd = Path::new(&info.path);
    // Spawning git in a vanished directory would be reported as a missing git executable.
    if !cwd.is_dir() {
        return Err(AppError::msg(format!("not found: {}", info.path)));
    }
    let has_worktree = !info.is_bare;
    let with_submodules = has_worktree && cwd.join(".gitmodules").is_file();
    let (refs, head_oid, head_ref, stashes, worktrees, gitlinks, urls) = std::thread::scope(|s| {
        let head_oid = s.spawn(|| Git::new(cwd).args(["rev-parse", "-q", "--verify", "HEAD"]).allow_codes(&[1]).run());
        // Not `--short`: it yields `heads/x` when a tag `x` exists.
        let head_ref = s.spawn(|| Git::new(cwd).args(["symbolic-ref", "-q", "HEAD"]).allow_codes(&[1]).run());
        let stashes = s.spawn(|| Git::new(cwd).args(STASH_ARGS).run());
        let worktrees = s.spawn(|| Git::new(cwd).args(["worktree", "list", "--porcelain", "-z"]).run());
        let gitlinks = with_submodules.then(|| s.spawn(|| Git::new(cwd).args(["ls-files", "-s", "-z"]).run()));
        let urls = s.spawn(|| {
            Git::new(cwd).args(["config", "-z", "--get-regexp", r"^remote\..*\.url$"]).allow_codes(&[1]).run()
        });
        let refs = Git::new(cwd).arg("for-each-ref").arg(REF_FORMAT).args(REF_NAMESPACES).run();
        (refs, join(head_oid), join(head_ref), join(stashes), join(worktrees), gitlinks.map(join), join(urls))
    });
    let refs = refs?;
    let records = parse_for_each_ref(&refs);
    // Rare (custom fetch refspecs): only then a second, sequential process.
    let foreign = foreign_upstreams(&records);
    let lookup = (!foreign.is_empty()).then(|| {
        Git::new(cwd).args(["for-each-ref", "--format=%(refname)%00"]).args(foreign.iter().map(|u| lossy(u))).run()
    });
    let missing = missing_upstreams(&foreign, lookup);
    let head = parse_head(&head_oid?, &head_ref?);
    let stash_out = optional("stash list", Some(stashes));
    let worktree_out = optional("worktree list", Some(worktrees));
    let gitlink_out = optional("ls-files", gitlinks);
    let url_out = optional("remote urls", Some(urls));

    let configured = parse_remote_urls(&url_out);
    let names: Vec<&str> = configured.iter().map(|r| r.name.as_str()).collect();
    let (mut locals, remotes, tags) = collect_refs(&records, &names, &missing);
    let heads = remote_heads(&records, &names);
    let mut remote_list = merge_remote_list(configured, &remotes);
    for r in &mut remote_list {
        r.head = heads.iter().find(|(name, _)| *name == r.name).map(|(_, head)| head.clone());
    }

    let mut worktrees = parse_worktrees(&worktree_out);
    let resolved = resolve_worktrees(&mut worktrees, info);
    for branch in &mut locals {
        if let Some(raw) = branch.worktree.take() {
            branch.worktree = match resolved.get(&raw) {
                Some(path) => path.clone(),
                None => Some(repo::canonical(&raw)),
            };
        }
    }

    let submodules = parse_gitlinks(&gitlink_out)
        .into_iter()
        .map(|link| {
            let state = if link.conflict {
                "conflict"
            } else if cwd.join(&link.path).join(".git").exists() {
                "ok"
            } else {
                "uninitialized"
            };
            Submodule { path: link.path, oid: link.oid, state: state.to_string() }
        })
        .collect();

    Ok(RefsSnapshot {
        head,
        locals,
        remotes,
        remote_list,
        tags,
        stashes: parse_stashes(&stash_out),
        worktrees,
        submodules,
    })
}

/// Result of a scoped git worker; a panic becomes an error.
fn join<T>(handle: ScopedJoinHandle<'_, AppResult<T>>) -> AppResult<T> {
    handle.join().unwrap_or_else(|_| Err(AppError::msg("git worker thread panicked")))
}

/// Output of an auxiliary command (None = not run). A failure is logged and read as no output, so
/// one broken part (e.g. a corrupt stash reflog) does not hide every ref.
fn optional(what: &str, result: Option<AppResult<Vec<u8>>>) -> Vec<u8> {
    match result {
        Some(Ok(out)) => out,
        Some(Err(e)) => {
            eprintln!("[git-viewer] {what} failed: {e}");
            Vec::new()
        }
        None => Vec::new(),
    }
}

/// HEAD from `rev-parse -q --verify HEAD` (empty = unborn) and `symbolic-ref -q HEAD` (empty = detached).
pub fn parse_head(oid_out: &[u8], symref_out: &[u8]) -> HeadInfo {
    let oid = oid_out.trim_ascii();
    let symref = lossy(symref_out.trim_ascii());
    let branch = (!symref.is_empty()).then(|| symref.strip_prefix("refs/heads/").unwrap_or(&symref).to_string());
    HeadInfo {
        oid: (!oid.is_empty()).then(|| lossy(oid)),
        detached: branch.is_none() && !oid.is_empty(),
        unborn: oid.is_empty(),
        branch,
    }
}

/// One `for-each-ref` record; fields borrow the output buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RefRecord<'a> {
    pub refname: &'a [u8],
    pub objecttype: &'a [u8],
    pub objectname: &'a [u8],
    /// Type and oid after peeling tags; empty unless the ref points at a tag object.
    pub peeled_type: &'a [u8],
    pub peeled_name: &'a [u8],
    /// Checked out in the worktree git ran in (`%(HEAD)` is `*`).
    pub head: bool,
    /// Target of a symbolic ref (e.g. `refs/remotes/origin/HEAD`), else empty.
    pub symref: &'a [u8],
    /// Full upstream ref name, empty when none is configured.
    pub upstream: &'a [u8],
    pub worktree: &'a [u8],
    /// Tagger date of tag objects, committer date of commits; 0 when unknown.
    pub time: i64,
    pub subject: &'a [u8],
}

/// Records of a `for-each-ref` whose format ends each of its `N` fields with `%00`. Split on NUL
/// alone the output is a list of `N`-field records whose later first fields start with git's `\n`;
/// cutting records at `\0\n` is the same (no field can start with `\n`), and a malformed record (not
/// `N` fields, or an empty first one) is dropped without shifting the following ones.
fn ref_records<const N: usize>(buf: &[u8]) -> impl Iterator<Item = [&[u8]; N]> + '_ {
    let mut start = 0;
    memmem::find_iter(buf, b"\0\n").filter_map(move |end| {
        let record = &buf[start..end];
        start = end + 2;
        let mut f: [&[u8]; N] = [&b""[..]; N];
        let mut count = 0;
        for field in record.split(|b| *b == 0) {
            if count < N {
                f[count] = field;
            }
            count += 1;
        }
        (count == N && !f[0].is_empty()).then_some(f)
    })
}

/// Parse `for-each-ref --format=<REF_FORMAT>` output.
pub fn parse_for_each_ref(buf: &[u8]) -> Vec<RefRecord<'_>> {
    ref_records::<REF_FIELDS>(buf)
        .map(
            |[refname, objecttype, objectname, peeled_type, peeled_name, head, symref, upstream, worktree, time, subject]| {
                RefRecord {
                    refname,
                    objecttype,
                    objectname,
                    peeled_type,
                    peeled_name,
                    head: head == b"*",
                    symref,
                    upstream,
                    worktree,
                    time: parse_num(time),
                    subject,
                }
            },
        )
        .collect()
}

/// The ref lies in one of `REF_NAMESPACES` (what for-each-ref's pattern `refs/heads` matches).
fn in_namespaces(name: &[u8]) -> bool {
    REF_NAMESPACES.iter().any(|ns| name.strip_prefix(ns.as_bytes()).is_some_and(|rest| rest.first() == Some(&b'/')))
}

/// Distinct upstreams of `records` outside `REF_NAMESPACES`: the records cannot tell whether they exist.
fn foreign_upstreams<'a>(records: &[RefRecord<'a>]) -> Vec<&'a [u8]> {
    let mut out: Vec<&[u8]> =
        records.iter().map(|r| r.upstream).filter(|u| !u.is_empty() && !in_namespaces(u)).collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// Which of `foreign` do not exist, from `for-each-ref --format=%(refname)%00 <foreign…>` (None: not
/// run). A failed lookup marks none of them gone.
fn missing_upstreams<'a>(foreign: &[&'a [u8]], lookup: Option<AppResult<Vec<u8>>>) -> HashSet<&'a [u8]> {
    let out = match lookup {
        Some(Ok(out)) => out,
        Some(Err(e)) => {
            eprintln!("[git-viewer] upstream lookup failed: {e}");
            return HashSet::new();
        }
        None => return HashSet::new(),
    };
    // A pattern also matches the refs below it: compare exact names.
    let found: HashSet<&[u8]> = ref_records::<1>(&out).map(|[name]| name).collect();
    foreign.iter().copied().filter(|u| !found.contains(u)).collect()
}

/// Locals, remotes and tags from `for-each-ref` records, symrefs skipped, sorted for display: locals
/// by name, remotes by remote then branch (case-insensitive), tags newest first. `remote_names` are
/// the configured remotes used to split `refs/remotes/<remote>/<branch>`.
/// `LocalBranch::worktree` keeps git's raw `%(worktreepath)`; `snapshot` resolves it.
/// An upstream is gone when it has no record, or (outside `REF_NAMESPACES`) is in `missing`.
/// Ahead/behind are left 0: `track` counts them.
pub fn collect_refs(
    records: &[RefRecord<'_>],
    remote_names: &[&str],
    missing: &HashSet<&[u8]>,
) -> (Vec<LocalBranch>, Vec<RemoteBranch>, Vec<Tag>) {
    let listed: HashSet<&[u8]> = records.iter().map(|r| r.refname).collect();
    let gone = |up: &[u8]| !up.is_empty() && if in_namespaces(up) { !listed.contains(up) } else { missing.contains(up) };
    let mut locals = Vec::new();
    let mut remotes = Vec::new();
    let mut tags = Vec::new();
    for r in records {
        if !r.symref.is_empty() {
            continue;
        }
        let full_name = lossy(r.refname);
        let oid = lossy(if r.peeled_name.is_empty() { r.objectname } else { r.peeled_name });
        if let Some(rest) = full_name.strip_prefix("refs/heads/") {
            let name = rest.to_string();
            locals.push(LocalBranch {
                name,
                full_name,
                oid,
                upstream: short_upstream(r.upstream),
                ahead: 0,
                behind: 0,
                upstream_gone: gone(r.upstream),
                is_head: r.head,
                worktree: (!r.worktree.is_empty()).then(|| lossy(r.worktree)),
                time: r.time,
            });
        } else if let Some(rest) = full_name.strip_prefix("refs/remotes/") {
            let (remote, short) = split_remote_ref(rest, remote_names);
            let (remote, short, name) = (remote.to_string(), short.to_string(), rest.to_string());
            remotes.push(RemoteBranch { remote, name, short, full_name, oid, time: r.time });
        } else if let Some(rest) = full_name.strip_prefix("refs/tags/") {
            let name = rest.to_string();
            let annotated = r.objecttype == b"tag";
            let message = (annotated && !r.subject.is_empty()).then(|| lossy(r.subject));
            let target_type = lossy(if r.peeled_type.is_empty() { r.objecttype } else { r.peeled_type });
            tags.push(Tag { name, full_name, oid, target_type, annotated, message, time: r.time });
        }
    }
    locals.sort_by(|a, b| cmp_name(&a.name, &b.name));
    remotes.sort_by(|a, b| cmp_name(&a.remote, &b.remote).then_with(|| cmp_name(&a.short, &b.short)));
    tags.sort_by(|a, b| b.time.cmp(&a.time).then_with(|| cmp_name(&a.name, &b.name)));
    (locals, remotes, tags)
}

/// The remotes' default branches: for each symref `refs/remotes/<remote>/HEAD` among the `for-each-ref`
/// records (which `collect_refs` skips), the remote and the full name of the branch it points to — one
/// of that remote's own (`refs/remotes/<remote>/<branch>`). `remote_names` split the ref name as in
/// `collect_refs`.
pub fn remote_heads(records: &[RefRecord<'_>], remote_names: &[&str]) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for r in records.iter().filter(|r| !r.symref.is_empty()) {
        let Some(rest) = r.refname.strip_prefix(b"refs/remotes/") else { continue };
        let rest = lossy(rest);
        let (remote, branch) = split_remote_ref(&rest, remote_names);
        if branch != "HEAD" || remote == rest {
            continue;
        }
        let target = lossy(r.symref);
        let own = format!("refs/remotes/{remote}/");
        if target.len() > own.len() && target.starts_with(&own) {
            out.push((remote.to_string(), target));
        }
    }
    out
}

/// Split `<remote>/<branch>` (a ref name after `refs/remotes/`) at the longest configured remote name
/// it starts with (names may contain `/`), else at the first `/`. Without any `/` both parts are `rest`.
pub fn split_remote_ref<'a>(rest: &'a str, remote_names: &[&str]) -> (&'a str, &'a str) {
    let at = remote_names
        .iter()
        .filter(|r| {
            !r.is_empty() && rest.len() > r.len() + 1 && rest.starts_with(**r) && rest.as_bytes()[r.len()] == b'/'
        })
        .map(|r| r.len())
        .max()
        .or_else(|| rest.find('/'));
    match at {
        Some(i) => (&rest[..i], &rest[i + 1..]),
        None => (rest, rest),
    }
}

/// Upstream display name: `refs/remotes/origin/x` → `origin/x`, `refs/heads/x` → `x` (local upstream).
fn short_upstream(full: &[u8]) -> Option<String> {
    if full.is_empty() {
        return None;
    }
    let s = lossy(full);
    let short = ["refs/remotes/", "refs/heads/", "refs/"].iter().find_map(|p| s.strip_prefix(*p));
    Some(short.map_or_else(|| s.clone(), str::to_string))
}

/// Ahead/behind counts relative to the upstream.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Track {
    pub ahead: u32,
    pub behind: u32,
    /// Upstream configured but its ref is gone.
    pub gone: bool,
}

/// Parse `%(upstream:track,nobracket)`: ``, `ahead N`, `behind N`, `ahead N, behind M` or `gone`.
pub fn parse_track(text: &[u8]) -> Track {
    let mut t = Track::default();
    for part in text.split(|b| *b == b',') {
        let part = part.trim_ascii();
        if part == b"gone" {
            t.gone = true;
        } else if let Some(n) = part.strip_prefix(b"ahead ") {
            t.ahead = parse_num(n);
        } else if let Some(n) = part.strip_prefix(b"behind ") {
            t.behind = parse_num(n);
        }
    }
    t
}

// ───────────── ahead/behind ─────────────

/// Local branches: name, tip, full upstream name (`%(upstream)` walks nothing).
const BRANCH_FORMAT: &str = "--format=%(refname)%00%(objectname)%00%(upstream)%00";
/// Name and tip of the upstream refs.
const TIP_FORMAT: &str = "--format=%(refname)%00%(objectname)%00";
/// git's own count, one walk per branch (see `parse_track`).
const TRACK_FORMAT: &str = "--format=%(refname)%00%(objectname)%00%(upstream:track,nobracket)%00";
/// Ref names per git command: far below the argument size limit.
const REFS_PER_CALL: usize = 1000;

/// Ahead/behind of every local branch whose upstream exists, sorted by name. Apart from `snapshot`,
/// which every refresh and graph load waits for: the walks can take seconds on a big history.
/// `known` is an earlier answer; a branch whose (tip, upstream tip) pair it holds is not walked again,
/// so a refresh that moved nothing costs two cheap `for-each-ref` listings.
///
/// Branches with the same upstream tip (typically many tracking the default branch) are counted by
/// one `%(ahead-behind:<tip>)` together: one walk instead of one per branch (0.4 s instead of 3 s
/// for 136 branches up to 490k commits behind). The others go to one `%(upstream:track)` run, whose
/// walk per branch is all a lone branch needs; so do all of them when the grouped count fails (git
/// before 2.41 has no `%(ahead-behind:)`).
pub fn track(cwd: &Path, known: &[BranchTrack]) -> AppResult<Vec<BranchTrack>> {
    let listing = Git::new(cwd).args(["for-each-ref", BRANCH_FORMAT, "refs/heads"]).run()?;
    let branches = parse_tracked_branches(&listing);
    if branches.is_empty() {
        return Ok(Vec::new());
    }
    let tips = upstream_tips(cwd, &branches)?;
    let (mut done, walks) = plan_track(&branches, &tips, known);
    let mut grouped = true;
    // (branch, upstream tip) pairs left for the walk per branch.
    let mut each: Vec<(String, String)> = Vec::new();
    for (tip, names) in walks {
        if grouped && names.len() > 1 {
            match ahead_behind(cwd, &tip, &names) {
                Ok(counts) => {
                    done.extend(counts);
                    continue;
                }
                Err(e) => {
                    eprintln!("[git-viewer] ahead-behind failed, counting per branch: {e}");
                    grouped = false;
                }
            }
        }
        each.extend(names.into_iter().map(|name| (name, tip.clone())));
    }
    if !each.is_empty() {
        done.extend(track_each(cwd, &each)?);
    }
    done.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(done)
}

/// A local branch with an upstream configured.
#[derive(Debug, Clone, PartialEq, Eq)]
struct TrackedBranch {
    name: String,
    oid: String,
    /// Full upstream ref name.
    upstream: String,
}

/// Parse `for-each-ref --format=<BRANCH_FORMAT>`: the branches with an upstream.
fn parse_tracked_branches(buf: &[u8]) -> Vec<TrackedBranch> {
    ref_records::<3>(buf)
        .filter(|[_, _, upstream]| !upstream.is_empty())
        .map(|[name, oid, upstream]| TrackedBranch { name: lossy(name), oid: lossy(oid), upstream: lossy(upstream) })
        .collect()
}

/// Upstream ref → tip, for the upstreams of `branches` that exist.
fn upstream_tips(cwd: &Path, branches: &[TrackedBranch]) -> AppResult<HashMap<String, String>> {
    let mut wanted: Vec<&str> = branches.iter().map(|b| b.upstream.as_str()).collect();
    wanted.sort_unstable();
    wanted.dedup();
    let mut tips = HashMap::new();
    for chunk in wanted.chunks(REFS_PER_CALL) {
        let out = Git::new(cwd).args(["for-each-ref", TIP_FORMAT]).args(chunk).run()?;
        // A pattern also matches the refs below it; lookups are by exact name.
        tips.extend(ref_records::<2>(&out).map(|[name, oid]| (lossy(name), lossy(oid))));
    }
    Ok(tips)
}

/// Split `branches` into counts reused from `known` (the counts depend on the two tips alone) and the
/// branches to walk, by upstream tip. A branch whose upstream is missing (gone) gets no counts.
fn plan_track(
    branches: &[TrackedBranch],
    tips: &HashMap<String, String>,
    known: &[BranchTrack],
) -> (Vec<BranchTrack>, BTreeMap<String, Vec<String>>) {
    let cached: HashMap<(&str, &str), (u32, u32)> =
        known.iter().map(|t| ((t.oid.as_str(), t.upstream_oid.as_str()), (t.ahead, t.behind))).collect();
    let mut done = Vec::new();
    let mut walks: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for b in branches {
        let Some(tip) = tips.get(&b.upstream) else { continue };
        match cached.get(&(b.oid.as_str(), tip.as_str())) {
            Some(&(ahead, behind)) => done.push(BranchTrack {
                name: b.name.clone(),
                oid: b.oid.clone(),
                upstream_oid: tip.clone(),
                ahead,
                behind,
            }),
            None => walks.entry(tip.clone()).or_default().push(b.name.clone()),
        }
    }
    (done, walks)
}

/// Counts of the branches `names` against `tip`, all from one `%(ahead-behind:<tip>)` walk.
fn ahead_behind(cwd: &Path, tip: &str, names: &[String]) -> AppResult<Vec<BranchTrack>> {
    let format = format!("--format=%(refname)%00%(objectname)%00%(ahead-behind:{tip})%00");
    let wanted: HashSet<&str> = names.iter().map(String::as_str).collect();
    let mut out = Vec::new();
    for chunk in names.chunks(REFS_PER_CALL) {
        let res = Git::new(cwd).arg("for-each-ref").arg(&format).args(chunk).run()?;
        out.extend(parse_ahead_behind(&res, tip, &wanted));
    }
    Ok(out)
}

/// Parse `for-each-ref --format=%(refname)%00%(objectname)%00%(ahead-behind:<tip>)%00` (`<ahead>
/// <behind>`, empty for a ref that is no commit): the branches in `wanted`.
fn parse_ahead_behind(buf: &[u8], tip: &str, wanted: &HashSet<&str>) -> Vec<BranchTrack> {
    ref_records::<3>(buf)
        .filter_map(|[name, oid, counts]| {
            let name = lossy(name);
            if !wanted.contains(name.as_str()) {
                return None;
            }
            let space = memchr(b' ', counts)?;
            let (ahead, behind) = (count(&counts[..space])?, count(&counts[space + 1..])?);
            Some(BranchTrack { name, oid: lossy(oid), upstream_oid: tip.to_string(), ahead, behind })
        })
        .collect()
}

/// Counts of `each` (branch, upstream tip read before) by `%(upstream:track)`, one walk per branch.
/// The walk reads the upstream again: should a fetch move it meanwhile, the newer counts are filed
/// under the older tip, a pair that then matches nothing and is counted again on the next call.
fn track_each(cwd: &Path, each: &[(String, String)]) -> AppResult<Vec<BranchTrack>> {
    let tip_of: HashMap<&str, &str> = each.iter().map(|(name, tip)| (name.as_str(), tip.as_str())).collect();
    let mut out = Vec::new();
    for chunk in each.chunks(REFS_PER_CALL) {
        let names = chunk.iter().map(|(name, _)| name);
        let res = Git::new(cwd).args(["for-each-ref", TRACK_FORMAT]).args(names).run()?;
        out.extend(parse_track_each(&res, &tip_of));
    }
    Ok(out)
}

/// Parse `for-each-ref --format=<TRACK_FORMAT>`: the branches of `tip_of` whose upstream is still there.
fn parse_track_each(buf: &[u8], tip_of: &HashMap<&str, &str>) -> Vec<BranchTrack> {
    ref_records::<3>(buf)
        .filter_map(|[name, oid, track]| {
            let name = lossy(name);
            let tip = tip_of.get(name.as_str())?.to_string();
            let t = parse_track(track);
            (!t.gone).then(|| BranchTrack { name, oid: lossy(oid), upstream_oid: tip, ahead: t.ahead, behind: t.behind })
        })
        .collect()
}

/// A decimal count; None when empty or malformed.
fn count(b: &[u8]) -> Option<u32> {
    std::str::from_utf8(b).ok()?.parse().ok()
}

/// Parse `STASH_ARGS` (= `stash list -z --format=<STASH_FORMAT>`): a flat list of NUL-terminated fields, 7 per
/// stash (`%H`, `%P`, `%gd`, `%at`, `%aN`, `%aE`, `%gs`), newest first. Parents: base, index commit,
/// untracked commit.
pub fn parse_stashes(buf: &[u8]) -> Vec<Stash> {
    let fields = split_records(buf, 0);
    let (records, _) = fields.as_chunks::<STASH_FIELDS>();
    records
        .iter()
        .enumerate()
        .filter_map(|(i, [oid, parents, selector, time, author, email, message])| {
            let mut parents = parents.split(|b| *b == b' ').filter(|p| !p.is_empty()).map(lossy);
            let base = parents.next()?;
            let index = selector_index(selector).unwrap_or(i as u32);
            Some(Stash {
                index,
                name: format!("stash@{{{index}}}"),
                oid: lossy(oid),
                base,
                index_commit: parents.next(),
                untracked_commit: parents.next(),
                message: lossy(message),
                author: lossy(author),
                email: lossy(email),
                time: parse_num(time),
            })
        })
        .collect()
}

/// N from a `stash@{N}` reflog selector.
fn selector_index(selector: &[u8]) -> Option<u32> {
    let open = memchr(b'{', selector)? + 1;
    let len = memchr(b'}', &selector[open..])?;
    std::str::from_utf8(&selector[open..open + len]).ok()?.parse().ok()
}

/// Parse `worktree list --porcelain -z`: blocks of NUL-terminated `key[ value]` fields (`worktree`,
/// `HEAD`, `branch`, `detached`, `bare`, `locked[ reason]`, `prunable[ reason]`) ended by an empty
/// field; the first block is the main worktree. Paths are git's; `is_current` is left false.
pub fn parse_worktrees(buf: &[u8]) -> Vec<Worktree> {
    let mut out: Vec<Worktree> = Vec::new();
    let mut cur: Option<Worktree> = None;
    for field in buf.split(|b| *b == 0) {
        if field.is_empty() {
            out.extend(cur.take());
            continue;
        }
        let (key, value) = match memchr(b' ', field) {
            Some(i) => (&field[..i], &field[i + 1..]),
            None => (field, &field[field.len()..]),
        };
        if key == b"worktree" {
            out.extend(cur.take());
            cur = Some(Worktree {
                path: lossy(value),
                head: None,
                branch: None,
                detached: false,
                bare: false,
                locked: false,
                prunable: false,
                is_main: out.is_empty(),
                is_current: false,
            });
            continue;
        }
        let Some(w) = cur.as_mut() else { continue };
        match key {
            // Unborn HEAD is printed as the null oid.
            b"HEAD" => w.head = value.iter().any(|b| *b != b'0').then(|| lossy(value)),
            b"branch" => w.branch = Some(lossy(value.strip_prefix(b"refs/heads/").unwrap_or(value))),
            b"detached" => w.detached = true,
            b"bare" => w.bare = true,
            b"locked" => w.locked = true,
            b"prunable" => w.prunable = true,
            _ => {}
        }
    }
    out.extend(cur);
    out
}

/// Canonicalize worktree paths, mark the opened one and name the branch of worktrees a rebase or
/// bisect detached. Returns git's raw path → resolved path (None for a bare main entry, which is no
/// checkout), used to resolve `%(worktreepath)` identically.
fn resolve_worktrees(list: &mut [Worktree], info: &RepoInfo) -> HashMap<String, Option<String>> {
    let mut resolved = HashMap::with_capacity(list.len());
    for w in list.iter_mut() {
        let raw = std::mem::take(&mut w.path);
        let mut path = repo::canonical(&raw);
        // git reports a main worktree whose git dir lives elsewhere (submodule, core.worktree) as that
        // git dir; when it is the opened worktree its real path is known.
        if w.is_main && !w.bare && path == info.common_dir && info.git_dir == info.common_dir {
            path = info.path.clone();
        }
        w.is_current = path == info.path;
        if w.detached && w.branch.is_none() && !w.bare {
            w.branch = worktree_git_dir(w, &path, info).and_then(|dir| paused_branch(&dir));
        }
        resolved.insert(raw, (!w.bare).then(|| path.clone()));
        w.path = path;
    }
    resolved
}

/// Git dir of a worktree: known for the opened and the main one; a linked worktree's `.git` file names
/// it (`gitdir: <path>`, relative to the worktree with `worktree.useRelativePaths`).
fn worktree_git_dir(w: &Worktree, path: &str, info: &RepoInfo) -> Option<PathBuf> {
    if w.is_current {
        return Some(PathBuf::from(&info.git_dir));
    }
    if w.is_main {
        return Some(PathBuf::from(&info.common_dir));
    }
    repo::gitfile_target(Path::new(path))
}

/// Branch of a worktree whose HEAD is detached by a rebase or bisect in progress.
fn paused_branch(git_dir: &Path) -> Option<String> {
    ["rebase-merge/head-name", "rebase-apply/head-name", "BISECT_START"]
        .iter()
        .find_map(|f| std::fs::read(git_dir.join(f)).ok())
        .and_then(|text| repo::branch_from_head_name(&lossy(&text)))
}

/// A gitlink (submodule) entry of the index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Gitlink {
    pub path: String,
    /// Recorded commit; "ours" (stage 2) when unmerged.
    pub oid: String,
    /// Unmerged (stage 1–3 entries).
    pub conflict: bool,
}

/// Gitlinks (mode 160000) from `ls-files -s -z` (`<mode> <oid> <stage>\t<path>\0`). The stage entries
/// of an unmerged path (adjacent, ascending) are merged into one conflicted entry.
pub fn parse_gitlinks(buf: &[u8]) -> Vec<Gitlink> {
    let mut out: Vec<Gitlink> = Vec::new();
    let mut last_path: &[u8] = &[];
    for record in buf.split(|b| *b == 0) {
        // The index can hold 100k+ entries: skip everything but gitlinks before looking further.
        let Some(rest) = record.strip_prefix(b"160000 ") else { continue };
        let Some(tab) = memchr(b'\t', rest) else { continue };
        let (meta, path) = (&rest[..tab], &rest[tab + 1..]);
        let Some(space) = memchr(b' ', meta) else { continue };
        let (oid, stage) = (&meta[..space], &meta[space + 1..]);
        let unmerged = stage != b"0";
        if let Some(prev) = out.last_mut().filter(|_| unmerged && path == last_path) {
            prev.conflict = true;
            if stage == b"2" {
                prev.oid = lossy(oid);
            }
            continue;
        }
        last_path = path;
        out.push(Gitlink { path: lossy(path), oid: lossy(oid), conflict: unmerged });
    }
    out
}

/// Remotes from `config -z --get-regexp ^remote\..*\.url$` (`remote.<name>.url\n<url>\0`; a key
/// without value has no `\n`). Names may contain `.` and `/`; the first url of a remote wins.
pub fn parse_remote_urls(buf: &[u8]) -> Vec<Remote> {
    let mut out: Vec<Remote> = Vec::new();
    for record in split_records(buf, 0) {
        let (key, url) = match memchr(b'\n', record) {
            Some(i) => (&record[..i], Some(&record[i + 1..])),
            None => (record, None),
        };
        let Some(name) = key.strip_prefix(b"remote.").and_then(|k| k.strip_suffix(b".url")) else { continue };
        if name.is_empty() {
            continue;
        }
        let name = lossy(name);
        let url = url.filter(|u| !u.is_empty()).map(lossy);
        match out.iter_mut().find(|r| r.name == name) {
            Some(r) => {
                if r.url.is_none() {
                    r.url = url;
                }
            }
            None => out.push(Remote { name, url, head: None }),
        }
    }
    out
}

/// Configured remotes plus every remote that only shows up through its branches, sorted by name.
pub fn merge_remote_list(mut list: Vec<Remote>, branches: &[RemoteBranch]) -> Vec<Remote> {
    for b in branches {
        if !list.iter().any(|r| r.name == b.remote) {
            list.push(Remote { name: b.remote.clone(), url: None, head: None });
        }
    }
    list.sort_by(|a, b| cmp_name(&a.name, &b.name));
    list
}

/// ASCII case-insensitive order, ties broken bytewise (deterministic).
fn cmp_name(a: &str, b: &str) -> Ordering {
    a.bytes().map(|c| c.to_ascii_lowercase()).cmp(b.bytes().map(|c| c.to_ascii_lowercase())).then_with(|| a.cmp(b))
}

/// Decimal field (unix time, counts); 0 when empty or malformed.
fn parse_num<T: std::str::FromStr + Default>(b: &[u8]) -> T {
    std::str::from_utf8(b).ok().and_then(|s| s.trim().parse().ok()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    // Real git 2.48 output; the scratch repo prefix is shortened to `/p` (paths that don't exist).

    /// `for-each-ref --format=<REF_FORMAT> refs/heads refs/remotes refs/tags`: branches with slashes,
    /// upstreams present and gone, remote `team/upstream` next to remote `team`, remote `up.stream`,
    /// symbolic `HEAD`s, lightweight/annotated/nested tags, tags on a tree and a blob.
    const FOR_EACH_REF: &[u8] = b"refs/heads/ahead-br\0commit\0e673652b559e39b6a8bae3243cc7d9303879ae02\0\0\0 \0\0refs/remotes/origin/ahead-br\0\x001700014400\0ahead commit\0\n\
        refs/heads/behind-br\0commit\x00608e2807b13e404c92f6da07575efeb41d5185f4\0\0\0 \0\0refs/remotes/origin/behind-br\0\x001700007200\0second\0\n\
        refs/heads/both-br\0commit\x005fbdb9e5eea0a816aca40e4a3d855b9a18a9c39d\0\0\0 \0\0refs/remotes/origin/both-br\0\x001700032400\0local both 2\0\n\
        refs/heads/feature/x\0commit\x00608e2807b13e404c92f6da07575efeb41d5185f4\0\0\0 \0\0refs/remotes/team/upstream/feature\0\x001700007200\0second\0\n\
        refs/heads/feature/y/z\0commit\x00608e2807b13e404c92f6da07575efeb41d5185f4\0\0\0 \0\0\0\x001700007200\0second\0\n\
        refs/heads/gone-br\0commit\x00608e2807b13e404c92f6da07575efeb41d5185f4\0\0\0 \0\0refs/remotes/origin/gone-br\0\x001700007200\0second\0\n\
        refs/heads/gone-wt-br\0commit\x00059ff47ea9238708e10fff477ab79221c20fac32\0\0\0 \0\0\0/p/wt-gone\x001700036000\0add submodules\0\n\
        refs/heads/main\0commit\x00059ff47ea9238708e10fff477ab79221c20fac32\0\0\0*\0\0refs/remotes/origin/main\0/p/repo\x001700036000\0add submodules\0\n\
        refs/heads/wt-branch\0commit\x00608e2807b13e404c92f6da07575efeb41d5185f4\0\0\0 \0\0\0/p/wt-linked\x001700007200\0second\0\n\
        refs/remotes/origin/HEAD\0commit\x00608e2807b13e404c92f6da07575efeb41d5185f4\0\0\0 \0refs/remotes/origin/main\0\0\x001700007200\0second\0\n\
        refs/remotes/origin/ahead-br\0commit\x00608e2807b13e404c92f6da07575efeb41d5185f4\0\0\0 \0\0\0\x001700007200\0second\0\n\
        refs/remotes/origin/behind-br\0commit\0bc83ede55650b39d92dda924d4e62540820f2c37\0\0\0 \0\0\0\x001700021600\0remote behind 2\0\n\
        refs/remotes/origin/both-br\0commit\0bacb73b0a97cb0d8bbc555f1f100e3ae2af5ab46\0\0\0 \0\0\0\x001700025200\0remote both\0\n\
        refs/remotes/origin/main\0commit\x00608e2807b13e404c92f6da07575efeb41d5185f4\0\0\0 \0\0\0\x001700007200\0second\0\n\
        refs/remotes/team/upstream/HEAD\0commit\x00608e2807b13e404c92f6da07575efeb41d5185f4\0\0\0 \0refs/remotes/team/upstream/main\0\0\x001700007200\0second\0\n\
        refs/remotes/team/upstream/feature\0commit\x00608e2807b13e404c92f6da07575efeb41d5185f4\0\0\0 \0\0\0\x001700007200\0second\0\n\
        refs/remotes/team/upstream/main\0commit\x00608e2807b13e404c92f6da07575efeb41d5185f4\0\0\0 \0\0\0\x001700007200\0second\0\n\
        refs/remotes/team/x\0commit\x00608e2807b13e404c92f6da07575efeb41d5185f4\0\0\0 \0\0\0\x001700007200\0second\0\n\
        refs/remotes/up.stream/dotted\0commit\x00608e2807b13e404c92f6da07575efeb41d5185f4\0\0\0 \0\0\0\x001700007200\0second\0\n\
        refs/tags/blob-light\0blob\x00422c2b7ab3b3c668038da977e4e93a5fc623169c\0\0\0 \0\0\0\0\0\0\n\
        refs/tags/light\0commit\x00608e2807b13e404c92f6da07575efeb41d5185f4\0\0\0 \0\0\0\x001700007200\0second\0\n\
        refs/tags/nested\0tag\x007d4604f06654048bf1a78ba1e8043ed765d85d22\0commit\x00608e2807b13e404c92f6da07575efeb41d5185f4\0 \0\0\0\x001700010800\0tag of tag\0\n\
        refs/tags/release/2.0\0tag\x004835f73defd21cd94497b641fe744d2eab4fb591\0commit\x00608e2807b13e404c92f6da07575efeb41d5185f4\0 \0\0\0\x001700010800\0slash tag second line of subject paragraph\0\n\
        refs/tags/tree-tag\0tag\x006fd35bdbd4931635e5bc41d5a884c57e2f6a29b1\0tree\x00187438d7d3fbee49dba13a50af1a6af1f1c7c17c\0 \0\0\0\x001700010800\0points at tree\0\n\
        refs/tags/v1.0\0tag\x001262fc5dd366a869fd09932242c726658d56c3f9\0commit\x00608e2807b13e404c92f6da07575efeb41d5185f4\0 \0\0\0\x001700010800\0Release 1.0\0\n";

    /// `config -z --get-regexp ^remote\..*\.url$` of the same repo.
    const REMOTE_URLS: &[u8] = b"remote.origin.url\n../origin.git\0\
        remote.team/upstream.url\n../team-up.git\0\
        remote.team.url\n../team.git\0\
        remote.up.stream.url\n../dot.git\0";

    /// A url key without value, a remote with two urls, a url containing a newline.
    const REMOTE_URLS_EDGE: &[u8] = b"remote.novalue.url\0\
        remote.multi.url\na\0\
        remote.multi.url\nb\0\
        remote.Nl.url\nline1\nline2\0";

    /// `stash list -z --format=<STASH_FORMAT>`: a default message, `-m` by another author, and `-u`
    /// (third parent).
    const STASHES: &[u8] = b"0a344d6367451ff2e206fe2490a816958258bc0d\x00059ff47ea9238708e10fff477ab79221c20fac32 e8d0676eb7c9e20e9423ac74beaac271c8a727ca\0stash@{0}\x001700046800\0A U Thor\0author@example.com\0WIP on main: 059ff47 add submodules\0\
        2de2a12cf81cea62c1added309de38346f24de31\x00059ff47ea9238708e10fff477ab79221c20fac32 f59effea847c26227998f1c8472488b178baf84b\0stash@{1}\x001700043200\0Ann Other\0ann@example.com\0On main: second\0\
        6f0a2b04705ecab287a730827c8b3c70aa23f5c8\x00059ff47ea9238708e10fff477ab79221c20fac32 67384156ce4c0c41684e001fd24e33eb3dd4e1a6 c29c54122a233133e4acf66da1e1d30a5dc2d975\0stash@{2}\x001700039600\0A U Thor\0author@example.com\0On main: wip with untracked\0";

    /// The same command for a stash made with a non-ASCII author name and an empty email (`<>`), whose
    /// `-m ""` falls back to the default message.
    const STASH_NO_EMAIL: &[u8] = b"e4b4b269944c92042f35a3f9cbe6757eae5e0864\0af54137c7a855e7ed180082e1740ad051af54447 5bce2c302c66b2ffe6cb378cca054dcc43593bad\0stash@{0}\x001794113278\0J\xc3\xbcrgen \xc3\x96\0\0WIP on main: af54137 second\0";

    /// `worktree list --porcelain -z`: main, a detached + locked worktree whose path has a space, a
    /// prunable one and a linked one.
    const WORKTREES: &[u8] = b"worktree /p/repo\0HEAD 059ff47ea9238708e10fff477ab79221c20fac32\0branch refs/heads/main\0\0\
        worktree /p/wt detached\0HEAD 7b46b4a51a02ab3cf0e21ec9838e3c40366a4560\0detached\0locked on usb\0\0\
        worktree /p/wt-gone\0HEAD 059ff47ea9238708e10fff477ab79221c20fac32\0branch refs/heads/gone-wt-br\0prunable gitdir file points to non-existent location\0\0\
        worktree /p/wt-linked\0HEAD 608e2807b13e404c92f6da07575efeb41d5185f4\0branch refs/heads/wt-branch\0\0";

    /// Run inside a submodule: the main worktree is reported as the submodule's git dir.
    const WORKTREES_SUBMODULE: &[u8] =
        b"worktree /p/repo/.git/modules/libs/sub\0HEAD fb73c1f5c4f039c0e64f45278bce0c05ae888d10\0branch refs/heads/main\0\0";
    const WORKTREES_BARE: &[u8] = b"worktree /p/bare.git\0bare\0\0";
    const WORKTREES_UNBORN: &[u8] =
        b"worktree /p/unborn\0HEAD 0000000000000000000000000000000000000000\0branch refs/heads/main\0\0";

    /// `ls-files -s -z` with an initialized and a deinitialized submodule.
    const LS_FILES: &[u8] = b"100644 4909e8445df26b2a0916acff57c6cf6c4bfc1c41 0\t.gitmodules\0\
        100644 422c2b7ab3b3c668038da977e4e93a5fc623169c 0\ta.txt\0\
        160000 fb73c1f5c4f039c0e64f45278bce0c05ae888d10 0\tlibs/sub\0\
        160000 7432c48788a1f83c0a41fbfd721a1391ab65fb9f 0\tlibs/sub two\0";

    /// `ls-files -s -z` during a merge with conflicting submodule commits.
    const LS_FILES_CONFLICT: &[u8] = b"100644 90ed8eb13d1f2ce3e8f0fa0c3e046455013b0364 0\t.gitmodules\0\
        100644 587be6b4c3f93f93c489c0111bba5596147a26cb 0\tf\0\
        160000 fb73c1f5c4f039c0e64f45278bce0c05ae888d10 1\tlibs/sub\0\
        160000 937d1900c58528b4d8a79fb25f25b13edcc8f3ca 2\tlibs/sub\0\
        160000 07838c5dc9a8942c4c2b484f6136ebb2cf6f93a9 3\tlibs/sub\0";

    /// `for-each-ref --format=<BRANCH_FORMAT> refs/heads` of a repo whose `main` (= origin/main), `cust`
    /// remote (refspec `+refs/heads/*:refs/custom/cust/*`) and origin/main are on A735: ahead2, behind,
    /// main → origin/main; localup → local main; custbr → refs/custom/cust/main; feat → origin/feat;
    /// gonebr and custgone → upstreams that no longer exist; noup → none.
    const BRANCH_LIST: &[u8] = b"refs/heads/ahead2\x0039727a6b28789d0424995b64d9d28c91e90fb6ed\0refs/remotes/origin/main\0\n\
        refs/heads/behind\x002b1433b5f4db1423b64b2742687cd045aff23dea\0refs/remotes/origin/main\0\n\
        refs/heads/custbr\0f9c77e886c9a7ac932cd0a92610c2beb819bd125\0refs/custom/cust/main\0\n\
        refs/heads/custgone\0a735d906a2f3427412fc2e6356f552ed2d94ce24\0refs/custom/cust/nothere\0\n\
        refs/heads/feat\0f862033310a6efee25455243adfd72bea214b527\0refs/remotes/origin/feat\0\n\
        refs/heads/gonebr\x002b1433b5f4db1423b64b2742687cd045aff23dea\0refs/remotes/origin/gonebr\0\n\
        refs/heads/localup\0a735d906a2f3427412fc2e6356f552ed2d94ce24\0refs/heads/main\0\n\
        refs/heads/main\0a735d906a2f3427412fc2e6356f552ed2d94ce24\0refs/remotes/origin/main\0\n\
        refs/heads/noup\0a735d906a2f3427412fc2e6356f552ed2d94ce24\0\0\n";

    /// `for-each-ref --format=<TIP_FORMAT>` with the upstreams of `BRANCH_LIST` as patterns.
    const UPSTREAM_TIPS: &[u8] = b"refs/custom/cust/main\0a735d906a2f3427412fc2e6356f552ed2d94ce24\0\n\
        refs/heads/main\0a735d906a2f3427412fc2e6356f552ed2d94ce24\0\n\
        refs/remotes/origin/feat\x00738f2f4d2bb48327c2d4506d60abe123808ecd73\0\n\
        refs/remotes/origin/main\0a735d906a2f3427412fc2e6356f552ed2d94ce24\0\n";

    /// `for-each-ref --format=%(refname)%00%(objectname)%00%(ahead-behind:<A735>)%00` over the branches
    /// whose upstream is on A735.
    const AHEAD_BEHIND: &[u8] = b"refs/heads/ahead2\x0039727a6b28789d0424995b64d9d28c91e90fb6ed\x002 0\0\n\
        refs/heads/behind\x002b1433b5f4db1423b64b2742687cd045aff23dea\x000 2\0\n\
        refs/heads/custbr\0f9c77e886c9a7ac932cd0a92610c2beb819bd125\x000 1\0\n\
        refs/heads/localup\0a735d906a2f3427412fc2e6356f552ed2d94ce24\x000 0\0\n\
        refs/heads/main\0a735d906a2f3427412fc2e6356f552ed2d94ce24\x000 0\0\n";

    /// `for-each-ref --format=<TRACK_FORMAT>` for bothbr (a commit on origin/main~1), behind, feat, gonebr.
    const TRACK_EACH: &[u8] = b"refs/heads/behind\x002b1433b5f4db1423b64b2742687cd045aff23dea\0behind 2\0\n\
        refs/heads/bothbr\x0051cb97dc07b8d0199b65a6b914ebe8910605f16a\0ahead 1, behind 1\0\n\
        refs/heads/feat\0f862033310a6efee25455243adfd72bea214b527\0ahead 1\0\n\
        refs/heads/gonebr\x002b1433b5f4db1423b64b2742687cd045aff23dea\0gone\0\n";

    const A735: &str = "a735d906a2f3427412fc2e6356f552ed2d94ce24";
    const F738: &str = "738f2f4d2bb48327c2d4506d60abe123808ecd73";

    const C608: &str = "608e2807b13e404c92f6da07575efeb41d5185f4";
    const C059: &str = "059ff47ea9238708e10fff477ab79221c20fac32";
    const REMOTES: &[&str] = &["origin", "team/upstream", "team", "up.stream"];

    fn info(path: &str, git_dir: &str, common_dir: &str, is_bare: bool) -> RepoInfo {
        RepoInfo {
            path: path.into(),
            name: String::new(),
            git_dir: git_dir.into(),
            common_dir: common_dir.into(),
            is_bare,
            is_linked_worktree: git_dir != common_dir,
        }
    }

    /// `collect_refs` of `for-each-ref` output whose upstreams all lie in `REF_NAMESPACES`.
    fn collect(buf: &[u8], remotes: &[&str]) -> (Vec<LocalBranch>, Vec<RemoteBranch>, Vec<Tag>) {
        collect_refs(&parse_for_each_ref(buf), remotes, &HashSet::new())
    }

    #[test]
    fn for_each_ref_records() {
        let recs = parse_for_each_ref(FOR_EACH_REF);
        assert_eq!(recs.len(), 25);
        assert_eq!(recs[0].refname, b"refs/heads/ahead-br");
        let main = recs.iter().find(|r| r.refname == b"refs/heads/main").unwrap();
        assert!(main.head);
        assert_eq!(main.upstream, b"refs/remotes/origin/main");
        assert_eq!(main.worktree, b"/p/repo");
        assert_eq!((main.time, main.subject), (1700036000, &b"add submodules"[..]));
        assert_eq!(recs.iter().filter(|r| r.head).count(), 1);
        let origin_head = recs.iter().find(|r| r.refname == b"refs/remotes/origin/HEAD").unwrap();
        assert_eq!(origin_head.symref, b"refs/remotes/origin/main");
        let nested = recs.iter().find(|r| r.refname == b"refs/tags/nested").unwrap();
        assert_eq!((nested.objecttype, nested.peeled_type), (&b"tag"[..], &b"commit"[..]));
        assert_eq!(nested.peeled_name, C608.as_bytes());
        let blob = recs.iter().find(|r| r.refname == b"refs/tags/blob-light").unwrap();
        assert_eq!((blob.objecttype, blob.time, blob.subject), (&b"blob"[..], 0, &b""[..]));
        assert_eq!(recs[24].refname, b"refs/tags/v1.0");
        assert_eq!(recs[24].subject, b"Release 1.0");
    }

    #[test]
    fn for_each_ref_empty_and_malformed() {
        assert!(parse_for_each_ref(b"").is_empty());
        let mut buf = b"refs/heads/bad\0commit\0\n".to_vec();
        buf.extend_from_slice(&FOR_EACH_REF[..FOR_EACH_REF.iter().position(|b| *b == b'\n').unwrap() + 1]);
        let recs = parse_for_each_ref(&buf);
        assert_eq!(recs.len(), 1);
        assert_eq!(recs[0].refname, b"refs/heads/ahead-br");
    }

    #[test]
    fn local_branches() {
        let (locals, _, _) = collect(FOR_EACH_REF, REMOTES);
        let names: Vec<&str> = locals.iter().map(|b| b.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "ahead-br",
                "behind-br",
                "both-br",
                "feature/x",
                "feature/y/z",
                "gone-br",
                "gone-wt-br",
                "main",
                "wt-branch"
            ]
        );
        let get = |n: &str| locals.iter().find(|b| b.name == n).unwrap();
        let ahead = get("ahead-br");
        assert_eq!(ahead.upstream.as_deref(), Some("origin/ahead-br"));
        assert!(!ahead.upstream_gone);
        assert_eq!(ahead.oid, "e673652b559e39b6a8bae3243cc7d9303879ae02");
        assert_eq!(ahead.time, 1700014400);
        // Counted by `track`, never by the snapshot.
        assert!(locals.iter().all(|b| b.ahead == 0 && b.behind == 0));
        let feature = get("feature/x");
        assert_eq!(feature.full_name, "refs/heads/feature/x");
        assert_eq!(feature.upstream.as_deref(), Some("team/upstream/feature"));
        assert!(!feature.upstream_gone);
        assert_eq!(get("feature/y/z").upstream, None);
        assert!(!get("feature/y/z").upstream_gone);
        // Its upstream has no record.
        let gone = get("gone-br");
        assert_eq!(gone.upstream.as_deref(), Some("origin/gone-br"));
        assert!(gone.upstream_gone);
        let gone: Vec<&str> = locals.iter().filter(|b| b.upstream_gone).map(|b| b.name.as_str()).collect();
        assert_eq!(gone, ["gone-br"]);
        let main = get("main");
        assert!(main.is_head);
        assert_eq!(main.oid, C059);
        assert_eq!(main.worktree.as_deref(), Some("/p/repo"));
        assert_eq!(get("wt-branch").worktree.as_deref(), Some("/p/wt-linked"));
        assert_eq!(get("gone-wt-br").worktree.as_deref(), Some("/p/wt-gone"));
        assert_eq!(get("feature/x").worktree, None);
        assert_eq!(locals.iter().filter(|b| b.is_head).count(), 1);
    }

    #[test]
    fn remote_branches_split_by_longest_configured_remote() {
        let (_, remotes, _) = collect(FOR_EACH_REF, REMOTES);
        let split: Vec<(&str, &str, &str)> =
            remotes.iter().map(|b| (b.remote.as_str(), b.short.as_str(), b.name.as_str())).collect();
        assert_eq!(
            split,
            [
                ("origin", "ahead-br", "origin/ahead-br"),
                ("origin", "behind-br", "origin/behind-br"),
                ("origin", "both-br", "origin/both-br"),
                ("origin", "main", "origin/main"),
                ("team", "x", "team/x"),
                ("team/upstream", "feature", "team/upstream/feature"),
                ("team/upstream", "main", "team/upstream/main"),
                ("up.stream", "dotted", "up.stream/dotted"),
            ]
        );
        let behind = remotes.iter().find(|b| b.name == "origin/behind-br").unwrap();
        assert_eq!(behind.full_name, "refs/remotes/origin/behind-br");
        assert_eq!((behind.oid.as_str(), behind.time), ("bc83ede55650b39d92dda924d4e62540820f2c37", 1700021600));
    }

    #[test]
    fn remote_branches_without_config_split_at_first_slash() {
        let (_, remotes, _) = collect(FOR_EACH_REF, &[]);
        let feature = remotes.iter().find(|b| b.name == "team/upstream/feature").unwrap();
        assert_eq!((feature.remote.as_str(), feature.short.as_str()), ("team", "upstream/feature"));
    }

    #[test]
    fn split_remote_names() {
        assert_eq!(split_remote_ref("origin/feature/x", &["origin"]), ("origin", "feature/x"));
        assert_eq!(split_remote_ref("a/b/c", &["a", "a/b"]), ("a/b", "c"));
        assert_eq!(split_remote_ref("a/b", &["a/b"]), ("a", "b"));
        assert_eq!(split_remote_ref("origin-2/x", &["origin"]), ("origin-2", "x"));
        assert_eq!(split_remote_ref("lonely", &["origin"]), ("lonely", "lonely"));
    }

    #[test]
    fn tags_newest_first_with_peeled_oids() {
        let (_, _, tags) = collect(FOR_EACH_REF, REMOTES);
        let names: Vec<&str> = tags.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, ["nested", "release/2.0", "tree-tag", "v1.0", "light", "blob-light"]);
        let get = |n: &str| tags.iter().find(|t| t.name == n).unwrap();
        let v1 = get("v1.0");
        assert!(v1.annotated);
        assert_eq!((v1.oid.as_str(), v1.message.as_deref(), v1.time), (C608, Some("Release 1.0"), 1700010800));
        assert_eq!(v1.full_name, "refs/tags/v1.0");
        assert_eq!(get("nested").oid, C608);
        assert_eq!(get("release/2.0").message.as_deref(), Some("slash tag second line of subject paragraph"));
        let tree = get("tree-tag");
        assert_eq!((tree.oid.as_str(), tree.annotated), ("187438d7d3fbee49dba13a50af1a6af1f1c7c17c", true));
        let light = get("light");
        assert_eq!(
            (light.oid.as_str(), light.annotated, light.message.as_deref(), light.time),
            (C608, false, None, 1700007200)
        );
        let blob = get("blob-light");
        assert_eq!(
            (blob.oid.as_str(), blob.annotated, blob.time),
            ("422c2b7ab3b3c668038da977e4e93a5fc623169c", false, 0)
        );
    }

    #[test]
    fn tag_target_types() {
        let (_, _, tags) = collect(FOR_EACH_REF, REMOTES);
        let types: Vec<(&str, &str)> = tags.iter().map(|t| (t.name.as_str(), t.target_type.as_str())).collect();
        // Peeled type for annotated tags (also through a tag of a tag), the object's own type otherwise.
        assert_eq!(
            types,
            [
                ("nested", "commit"),
                ("release/2.0", "commit"),
                ("tree-tag", "tree"),
                ("v1.0", "commit"),
                ("light", "commit"),
                ("blob-light", "blob"),
            ]
        );
        // Older git peels `%(*…)` one level only: a tag of a tag then names the inner tag object.
        let (outer, inner) = ("7d4604f06654048bf1a78ba1e8043ed765d85d22", "1262fc5dd366a869fd09932242c726658d56c3f9");
        let rec = format!("refs/tags/nested\0tag\0{outer}\0tag\0{inner}\0 \0\0\0\x001700010800\0tag of tag\0\n");
        let (_, _, tags) = collect(rec.as_bytes(), &[]);
        assert_eq!(tags.len(), 1);
        assert_eq!((tags[0].oid.as_str(), tags[0].target_type.as_str(), tags[0].annotated), (inner, "tag", true));
    }

    #[test]
    fn names_sort_case_insensitively() {
        let line = |name: &str| format!("refs/heads/{name}\0commit\0{C608}\0\0\0 \0\0\0\x001700000000\0s\0\n");
        let buf: String = ["Alpha", "Zeta", "alpha", "beta"].iter().map(|n| line(n)).collect();
        let (locals, _, _) = collect(buf.as_bytes(), &[]);
        let names: Vec<&str> = locals.iter().map(|b| b.name.as_str()).collect();
        assert_eq!(names, ["Alpha", "alpha", "beta", "Zeta"]);
    }

    #[test]
    fn track_text() {
        assert_eq!(parse_track(b""), Track::default());
        assert_eq!(parse_track(b"ahead 12"), Track { ahead: 12, behind: 0, gone: false });
        assert_eq!(parse_track(b"behind 3"), Track { ahead: 0, behind: 3, gone: false });
        assert_eq!(parse_track(b"ahead 2, behind 1"), Track { ahead: 2, behind: 1, gone: false });
        assert_eq!(parse_track(b"gone"), Track { ahead: 0, behind: 0, gone: true });
    }

    #[test]
    fn upstreams_outside_the_listed_namespaces_are_looked_up() {
        let line = |name: &str, up: &str| format!("refs/heads/{name}\0commit\0{C608}\0\0\0 \0\0{up}\0\x001700000000\0s\0\n");
        let buf = [
            line("custbr", "refs/custom/cust/main"),
            line("custgone", "refs/custom/cust/nothere"),
            line("local", "refs/heads/custbr"),
            line("tagged", "refs/tags/v9"),
        ]
        .concat();
        let records = parse_for_each_ref(buf.as_bytes());
        let foreign = foreign_upstreams(&records);
        assert_eq!(foreign, [&b"refs/custom/cust/main"[..], b"refs/custom/cust/nothere"]);
        // `for-each-ref --format=%(refname)%00 <foreign…>`: only the existing one comes back.
        let missing = missing_upstreams(&foreign, Some(Ok(b"refs/custom/cust/main\0\n".to_vec())));
        let (locals, _, _) = collect_refs(&records, &[], &missing);
        let gone: Vec<&str> = locals.iter().filter(|b| b.upstream_gone).map(|b| b.name.as_str()).collect();
        // A listed namespace needs no lookup: refs/tags/v9 has no record.
        assert_eq!(gone, ["custgone", "tagged"]);
        // Unknown when the lookup fails or did not run: not gone.
        assert!(missing_upstreams(&foreign, Some(Err(AppError::msg("x")))).is_empty());
        assert!(missing_upstreams(&foreign, None).is_empty());
        assert!(in_namespaces(b"refs/heads/x") && in_namespaces(b"refs/remotes/o/x"));
        assert!(!in_namespaces(b"refs/headsx/y") && !in_namespaces(b"refs/heads") && !in_namespaces(b"refs/custom/x"));
    }

    fn branch_track(name: &str, oid: &str, upstream_oid: &str, ahead: u32, behind: u32) -> BranchTrack {
        BranchTrack { name: name.into(), oid: oid.into(), upstream_oid: upstream_oid.into(), ahead, behind }
    }

    #[test]
    fn tracked_branches_and_upstream_tips() {
        let branches = parse_tracked_branches(BRANCH_LIST);
        let names: Vec<&str> = branches.iter().map(|b| b.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "refs/heads/ahead2",
                "refs/heads/behind",
                "refs/heads/custbr",
                "refs/heads/custgone",
                "refs/heads/feat",
                "refs/heads/gonebr",
                "refs/heads/localup",
                "refs/heads/main"
            ]
        );
        assert_eq!(branches[2].upstream, "refs/custom/cust/main");
        assert_eq!(branches[2].oid, "f9c77e886c9a7ac932cd0a92610c2beb819bd125");
        let tips: Vec<(&[u8], &[u8])> = ref_records::<2>(UPSTREAM_TIPS).map(|[n, o]| (n, o)).collect();
        assert_eq!(tips.len(), 4);
        assert_eq!(tips[2], (&b"refs/remotes/origin/feat"[..], F738.as_bytes()));
    }

    #[test]
    fn plan_reuses_known_pairs_and_groups_the_rest_by_upstream_tip() {
        let branches = parse_tracked_branches(BRANCH_LIST);
        let tips: HashMap<String, String> =
            ref_records::<2>(UPSTREAM_TIPS).map(|[n, o]| (lossy(n), lossy(o))).collect();
        let ahead2 = "39727a6b28789d0424995b64d9d28c91e90fb6ed";
        let known = [
            branch_track("refs/heads/ahead2", ahead2, A735, 2, 0),
            // The counts belong to the two tips, whatever the branch is called now.
            branch_track("refs/heads/renamed", A735, A735, 0, 0),
            // behind's upstream has moved since.
            branch_track("refs/heads/behind", "2b1433b5f4db1423b64b2742687cd045aff23dea", F738, 9, 9),
        ];
        let (done, walks) = plan_track(&branches, &tips, &known);
        assert_eq!(
            done,
            [
                branch_track("refs/heads/ahead2", ahead2, A735, 2, 0),
                branch_track("refs/heads/localup", A735, A735, 0, 0),
                branch_track("refs/heads/main", A735, A735, 0, 0),
            ]
        );
        // Gone upstreams (gonebr, custgone) are not walked.
        let walks: Vec<(&str, Vec<&str>)> =
            walks.iter().map(|(tip, names)| (tip.as_str(), names.iter().map(String::as_str).collect())).collect();
        assert_eq!(
            walks,
            [(F738, vec!["refs/heads/feat"]), (A735, vec!["refs/heads/behind", "refs/heads/custbr"])]
        );
        // Nothing known: everything with an upstream that exists is walked.
        let (done, walks) = plan_track(&branches, &tips, &[]);
        assert!(done.is_empty());
        assert_eq!(walks.values().map(Vec::len).sum::<usize>(), 6);
        assert_eq!(walks[A735].len(), 5);
    }

    #[test]
    fn ahead_behind_counts() {
        let wanted: HashSet<&str> =
            ["refs/heads/ahead2", "refs/heads/behind", "refs/heads/custbr", "refs/heads/main"].into_iter().collect();
        let counts = parse_ahead_behind(AHEAD_BEHIND, A735, &wanted);
        let got: Vec<(&str, u32, u32)> = counts.iter().map(|t| (t.name.as_str(), t.ahead, t.behind)).collect();
        // localup was not asked for.
        assert_eq!(
            got,
            [("refs/heads/ahead2", 2, 0), ("refs/heads/behind", 0, 2), ("refs/heads/custbr", 0, 1), ("refs/heads/main", 0, 0)]
        );
        assert!(counts.iter().all(|t| t.upstream_oid == A735));
        assert_eq!(counts[2].oid, "f9c77e886c9a7ac932cd0a92610c2beb819bd125");
        // No counts for a ref that is no commit (empty field) or a malformed one.
        let odd = format!("refs/heads/main\0{A735}\0\0\nrefs/heads/ahead2\0{A735}\x002\0\n");
        assert!(parse_ahead_behind(odd.as_bytes(), A735, &wanted).is_empty());
    }

    #[test]
    fn track_each_counts() {
        let tip_of: HashMap<&str, &str> =
            [("refs/heads/bothbr", A735), ("refs/heads/feat", F738), ("refs/heads/gonebr", A735)].into_iter().collect();
        let counts = parse_track_each(TRACK_EACH, &tip_of);
        // behind was not asked for; gonebr's upstream went away meanwhile.
        assert_eq!(
            counts,
            [
                branch_track("refs/heads/bothbr", "51cb97dc07b8d0199b65a6b914ebe8910605f16a", A735, 1, 1),
                branch_track("refs/heads/feat", "f862033310a6efee25455243adfd72bea214b527", F738, 1, 0),
            ]
        );
    }

    #[test]
    fn upstream_names() {
        assert_eq!(short_upstream(b""), None);
        assert_eq!(short_upstream(b"refs/remotes/team/upstream/feature").as_deref(), Some("team/upstream/feature"));
        assert_eq!(short_upstream(b"refs/heads/main").as_deref(), Some("main"));
    }

    #[test]
    fn head_states() {
        let on_branch = parse_head(format!("{C059}\n").as_bytes(), b"refs/heads/feature/x\n");
        assert_eq!(on_branch.oid.as_deref(), Some(C059));
        assert_eq!(on_branch.branch.as_deref(), Some("feature/x"));
        assert!(!on_branch.detached && !on_branch.unborn);
        let detached = parse_head(b"7b46b4a51a02ab3cf0e21ec9838e3c40366a4560\n", b"");
        assert_eq!(detached.branch, None);
        assert!(detached.detached && !detached.unborn);
        let unborn = parse_head(b"", b"refs/heads/main\n");
        assert_eq!((unborn.oid, unborn.branch.as_deref()), (None, Some("main")));
        assert!(unborn.unborn && !unborn.detached);
    }

    #[test]
    fn stashes() {
        let s = parse_stashes(STASHES);
        assert_eq!(s.len(), 3);
        assert_eq!((s[0].index, s[0].name.as_str()), (0, "stash@{0}"));
        assert_eq!(s[0].oid, "0a344d6367451ff2e206fe2490a816958258bc0d");
        assert_eq!(s[0].base, C059);
        assert_eq!(s[0].index_commit.as_deref(), Some("e8d0676eb7c9e20e9423ac74beaac271c8a727ca"));
        assert_eq!(s[0].untracked_commit, None);
        assert_eq!((s[0].message.as_str(), s[0].time), ("WIP on main: 059ff47 add submodules", 1700046800));
        assert_eq!((s[0].author.as_str(), s[0].email.as_str()), ("A U Thor", "author@example.com"));
        assert_eq!((s[1].index, s[1].message.as_str()), (1, "On main: second"));
        assert_eq!((s[1].author.as_str(), s[1].email.as_str()), ("Ann Other", "ann@example.com"));
        assert_eq!((s[2].index, s[2].name.as_str()), (2, "stash@{2}"));
        assert_eq!(s[2].index_commit.as_deref(), Some("67384156ce4c0c41684e001fd24e33eb3dd4e1a6"));
        assert_eq!(s[2].untracked_commit.as_deref(), Some("c29c54122a233133e4acf66da1e1d30a5dc2d975"));
        assert_eq!((s[2].message.as_str(), s[2].time), ("On main: wip with untracked", 1700039600));
        assert_eq!((s[2].author.as_str(), s[2].email.as_str()), ("A U Thor", "author@example.com"));
        assert!(parse_stashes(b"").is_empty());
    }

    #[test]
    fn stash_author_without_email() {
        // The empty email field keeps the message in place.
        let s = parse_stashes(STASH_NO_EMAIL);
        assert_eq!(s.len(), 1);
        assert_eq!((s[0].author.as_str(), s[0].email.as_str()), ("Jürgen Ö", ""));
        assert_eq!((s[0].message.as_str(), s[0].time), ("WIP on main: af54137 second", 1794113278));
        assert_eq!(s[0].base, "af54137c7a855e7ed180082e1740ad051af54447");
        assert_eq!(s[0].index_commit.as_deref(), Some("5bce2c302c66b2ffe6cb378cca054dcc43593bad"));
        // A record cut short (6 of 7 fields) is dropped, not misread.
        let cut = memmem::find(STASH_NO_EMAIL, b"WIP on").unwrap();
        assert!(parse_stashes(&STASH_NO_EMAIL[..cut]).is_empty());
    }

    #[test]
    fn worktree_blocks() {
        let w = parse_worktrees(WORKTREES);
        assert_eq!(w.len(), 4);
        assert_eq!(
            (w[0].path.as_str(), w[0].head.as_deref(), w[0].branch.as_deref()),
            ("/p/repo", Some(C059), Some("main"))
        );
        assert!(w[0].is_main && !w[0].detached && !w[0].bare && !w[0].locked && !w[0].prunable && !w[0].is_current);
        assert_eq!((w[1].path.as_str(), w[1].branch.as_deref()), ("/p/wt detached", None));
        assert_eq!(w[1].head.as_deref(), Some("7b46b4a51a02ab3cf0e21ec9838e3c40366a4560"));
        assert!(w[1].detached && w[1].locked && !w[1].prunable && !w[1].is_main);
        assert_eq!((w[2].path.as_str(), w[2].branch.as_deref()), ("/p/wt-gone", Some("gone-wt-br")));
        assert!(w[2].prunable && !w[2].locked && !w[2].detached);
        assert_eq!((w[3].path.as_str(), w[3].branch.as_deref()), ("/p/wt-linked", Some("wt-branch")));
        assert!(!w[3].is_main);

        let bare = parse_worktrees(WORKTREES_BARE);
        assert_eq!(bare.len(), 1);
        assert!(bare[0].bare && bare[0].is_main);
        assert_eq!((bare[0].head.as_deref(), bare[0].branch.as_deref()), (None, None));

        let unborn = parse_worktrees(WORKTREES_UNBORN);
        assert_eq!((unborn[0].head.as_deref(), unborn[0].branch.as_deref()), (None, Some("main")));
        assert!(parse_worktrees(b"").is_empty());
    }

    #[test]
    fn worktrees_resolved_against_the_opened_one() {
        let opened = info("/p/wt-linked", "/p/repo/.git/worktrees/wt-linked", "/p/repo/.git", false);
        let mut w = parse_worktrees(WORKTREES);
        let map = resolve_worktrees(&mut w, &opened);
        let current: Vec<&str> = w.iter().filter(|w| w.is_current).map(|w| w.path.as_str()).collect();
        assert_eq!(current, ["/p/wt-linked"]);
        assert_eq!(w[1].branch, None);
        assert_eq!(map.get("/p/repo"), Some(&Some("/p/repo".to_string())));
        assert_eq!(map.get("/p/wt-gone"), Some(&Some("/p/wt-gone".to_string())));
    }

    #[test]
    fn submodule_main_worktree_reported_as_git_dir() {
        let git_dir = "/p/repo/.git/modules/libs/sub";
        let opened = info("/p/repo/libs/sub", git_dir, git_dir, false);
        let mut w = parse_worktrees(WORKTREES_SUBMODULE);
        let map = resolve_worktrees(&mut w, &opened);
        assert_eq!(w[0].path, "/p/repo/libs/sub");
        assert!(w[0].is_current && w[0].is_main);
        assert_eq!(map.get(git_dir), Some(&Some("/p/repo/libs/sub".to_string())));
    }

    #[test]
    fn bare_main_entry_is_not_a_checkout() {
        let opened = info("/p/bare.git", "/p/bare.git", "/p/bare.git", true);
        let mut w = parse_worktrees(WORKTREES_BARE);
        let map = resolve_worktrees(&mut w, &opened);
        assert!(w[0].is_current);
        assert_eq!(map.get("/p/bare.git"), Some(&None));
    }

    #[test]
    fn gitlinks() {
        let links = parse_gitlinks(LS_FILES);
        assert_eq!(
            links,
            [
                Gitlink {
                    path: "libs/sub".into(),
                    oid: "fb73c1f5c4f039c0e64f45278bce0c05ae888d10".into(),
                    conflict: false
                },
                Gitlink {
                    path: "libs/sub two".into(),
                    oid: "7432c48788a1f83c0a41fbfd721a1391ab65fb9f".into(),
                    conflict: false
                },
            ]
        );
        let conflicted = parse_gitlinks(LS_FILES_CONFLICT);
        assert_eq!(
            conflicted,
            [Gitlink {
                path: "libs/sub".into(),
                oid: "937d1900c58528b4d8a79fb25f25b13edcc8f3ca".into(),
                conflict: true
            }]
        );
        assert!(parse_gitlinks(b"").is_empty());
    }

    #[test]
    fn remote_urls() {
        let r = parse_remote_urls(REMOTE_URLS);
        let pairs: Vec<(&str, Option<&str>)> = r.iter().map(|r| (r.name.as_str(), r.url.as_deref())).collect();
        assert_eq!(
            pairs,
            [
                ("origin", Some("../origin.git")),
                ("team/upstream", Some("../team-up.git")),
                ("team", Some("../team.git")),
                ("up.stream", Some("../dot.git")),
            ]
        );
        let edge = parse_remote_urls(REMOTE_URLS_EDGE);
        let pairs: Vec<(&str, Option<&str>)> = edge.iter().map(|r| (r.name.as_str(), r.url.as_deref())).collect();
        assert_eq!(pairs, [("novalue", None), ("multi", Some("a")), ("Nl", Some("line1\nline2"))]);
        assert!(parse_remote_urls(b"").is_empty());
    }

    #[test]
    fn remote_default_branches() {
        let configured = parse_remote_urls(REMOTE_URLS);
        let names: Vec<&str> = configured.iter().map(|r| r.name.as_str()).collect();
        let heads = remote_heads(&parse_for_each_ref(FOR_EACH_REF), &names);
        let pairs: Vec<(&str, &str)> = heads.iter().map(|(r, h)| (r.as_str(), h.as_str())).collect();
        // `team/upstream/HEAD` belongs to remote `team/upstream`, not to `team` (its branch `upstream/HEAD`).
        assert_eq!(
            pairs,
            [("origin", "refs/remotes/origin/main"), ("team/upstream", "refs/remotes/team/upstream/main")]
        );
        // Not a HEAD, a HEAD pointing outside its remote, a remote unknown to the config (split at its
        // first `/`), and no remote at all.
        let sym = |name: &str, target: &str| {
            format!("refs/remotes/{name} commit {C608}     {target}   1700000000 s 
")
        };
        let buf = [
            sym("origin/alias", "refs/remotes/origin/main"),
            sym("origin/HEAD", "refs/remotes/team/main"),
            sym("origin/HEAD", "refs/remotes/origin/"),
            sym("fork/HEAD", "refs/remotes/fork/dev"),
            sym("HEAD", "refs/remotes/main"),
        ]
        .concat();
        let heads = remote_heads(&parse_for_each_ref(buf.as_bytes()), &names);
        assert_eq!(heads, [("fork".to_string(), "refs/remotes/fork/dev".to_string())]);
    }

    #[test]
    fn remote_list_includes_remotes_known_only_from_branches() {
        let configured = parse_remote_urls(b"remote.origin.url\nu1\0remote.Upstream.url\nu2\0");
        let names: Vec<&str> = configured.iter().map(|r| r.name.as_str()).collect();
        let line = |r: &str| format!("refs/remotes/{r}\0commit\0{C608}\0\0\0 \0\0\0\x001700000000\0s\0\n");
        let buf: String = ["origin/main", "zed/main", "zed/dev", "fork/x"].iter().map(|r| line(r)).collect();
        let (_, remotes, _) = collect(buf.as_bytes(), &names);
        let list = merge_remote_list(configured, &remotes);
        let pairs: Vec<(&str, Option<&str>)> = list.iter().map(|r| (r.name.as_str(), r.url.as_deref())).collect();
        assert_eq!(pairs, [("fork", None), ("origin", Some("u1")), ("Upstream", Some("u2")), ("zed", None)]);
    }
}
