//! Filesystem watcher per open repo: raw notify (FSEvents) + our own coalescer thread →
//! one `repo-changed` event per burst.
//!
//! The notify callback runs on the FSEvents run-loop thread and only forwards events. The
//! coalescer thread classifies paths into [`kind`] bits and emits after a trailing quiet window.
//!
//! Changes below gitignored paths are dropped (builds, `npm install`, logs). The ignored paths come
//! from the worktree's own status: `git::status` lists them (`--ignored=matching`) whenever a
//! watcher watches its cwd and hands them over with [`share_ignored`].

use crate::error::{AppError, AppResult};
use crate::model::{RepoChanged, RepoInfo};
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::HashSet;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter};

/// Change kinds: bits of the set returned by [`classify`].
pub mod kind {
    /// Working tree files (and `info/exclude`) outside the ignored paths.
    pub const WORKTREE: u16 = 1 << 0;
    /// The worktree's index.
    pub const INDEX: u16 = 1 << 1;
    /// The worktree's HEAD or its reflog.
    pub const HEAD: u16 = 1 << 2;
    /// `refs/**`, `packed-refs`, `shallow`.
    pub const REFS: u16 = 1 << 3;
    /// `refs/stash` or its reflog.
    pub const STASH: u16 = 1 << 4;
    /// Merge / rebase / cherry-pick / revert / bisect state.
    pub const STATE: u16 = 1 << 5;
    /// `config`, `config.worktree`.
    pub const CONFIG: u16 = 1 << 6;
    /// A worktree was added, removed, locked or moved, or another worktree's HEAD moved.
    pub const WORKTREES: u16 = 1 << 7;
    /// A `.gitignore` or `info/exclude` changed, so the ignored paths may have too. Always comes
    /// with WORKTREE and is emitted as "worktree".
    pub const IGNORES: u16 = 1 << 8;
    /// Events were dropped or the git dir itself changed: refresh everything.
    pub const RESCAN: u16 = 1 << 15;
}

/// Quiet window when only git-dir kinds are pending (git finishes its writes within a few ms).
const QUIET_GIT: Duration = Duration::from_millis(80);
/// Quiet window when worktree files (or a rescan) are pending: editors save via tmp file + rename.
const QUIET_WORKTREE: Duration = Duration::from_millis(250);
/// Emit at the latest this long after the first event of a burst, even while churn continues.
const MAX_DELAY: Duration = Duration::from_millis(1000);
/// Kinds that need the long quiet window.
const SLOW_KINDS: u16 = kind::WORKTREE | kind::IGNORES | kind::RESCAN;
/// Kinds after which the ignored paths may be stale: the ignore rules changed, the index changed
/// (a path below an ignored directory may be tracked now), or events were lost.
const STALE_IGNORED: u16 = kind::IGNORES | kind::INDEX | kind::RESCAN;

/// `repo-changed` kind names in sorted order with the bits each one covers (RESCAN covers all).
const NAMES: [(&str, u16); 7] = [
    ("config", kind::CONFIG),
    ("head", kind::HEAD),
    ("index", kind::INDEX),
    ("refs", kind::REFS | kind::WORKTREES),
    ("stash", kind::STASH),
    ("state", kind::STATE),
    ("worktree", kind::WORKTREE | kind::IGNORES),
];

enum Msg {
    Fs(notify::Result<Event>),
    /// Ignored paths listed by a status that started at the given time.
    SetIgnored(Instant, Arc<IgnoredPaths>),
    Stop,
}

/// Ignored files kept per worktree (in-tree builds list every object file): changes to the ones
/// past it are not filtered. Ignored directories are always kept.
const MAX_IGNORED_FILES: usize = 100_000;

/// Gitignored paths of one worktree, relative to its root: the `! <path>` records of
/// `git status --porcelain=v2 -z --ignored=matching`. A directory is listed (with a trailing `/`)
/// only when it is ignored itself (by a pattern or below an ignored parent) and holds no tracked
/// file; the ignored files of any other directory are listed one by one (verified, git 2.48).
#[derive(Debug, Default)]
pub struct IgnoredPaths {
    dirs: HashSet<PathBuf>,
    files: HashSet<PathBuf>,
}

impl IgnoredPaths {
    /// Add the path of one `!` record.
    pub fn insert(&mut self, path: &[u8]) {
        let (set, path) = match path.strip_suffix(b"/") {
            Some(dir) => (&mut self.dirs, dir),
            None if self.files.len() >= MAX_IGNORED_FILES => return,
            None => (&mut self.files, path),
        };
        if !path.is_empty() {
            set.insert(path_from_bytes(path));
        }
    }

    /// Whether a change at `rel` (relative to the worktree root) cannot change `git status`. O(depth).
    pub fn covers(&self, rel: &Path) -> bool {
        // Anything below an ignored directory, but not the directory itself: a `name/` pattern only
        // matches directories, so a file created in its place would be untracked.
        rel.ancestors().skip(1).any(|dir| self.dirs.contains(dir))
            // git reads a `.gitignore` even when that file is ignored itself.
            || (self.files.contains(rel) && rel.file_name() != Some(OsStr::new(".gitignore")))
    }
}

fn path_from_bytes(b: &[u8]) -> PathBuf {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        PathBuf::from(OsStr::from_bytes(b))
    }
    #[cfg(not(unix))]
    {
        PathBuf::from(String::from_utf8_lossy(b).into_owned())
    }
}

/// Coalescer inboxes of the running watchers by canonical worktree root, so that [`share_ignored`]
/// can reach them. A root briefly has two watchers when two opens of one repo race.
static INBOXES: Mutex<Vec<Inbox>> = Mutex::new(Vec::new());
static NEXT_INBOX: AtomicU64 = AtomicU64::new(0);

struct Inbox {
    id: u64,
    root: PathBuf,
    tx: Sender<Msg>,
}

fn inboxes() -> MutexGuard<'static, Vec<Inbox>> {
    INBOXES.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The canonical form of `cwd` when it is the root of a watched worktree: a status run there should
/// list the ignored paths and pass them to [`share_ignored`].
pub fn watched_root(cwd: &Path) -> Option<PathBuf> {
    let root = std::fs::canonicalize(cwd).ok()?;
    inboxes().iter().any(|inbox| inbox.root == root).then_some(root)
}

/// Hand the ignored paths listed by a status of the worktree at `root` (canonical) that started at
/// `taken` to that worktree's watchers. From then on they drop changes below those paths.
pub fn share_ignored(root: &Path, taken: Instant, ignored: IgnoredPaths) {
    let ignored = Arc::new(ignored);
    for inbox in inboxes().iter().filter(|inbox| inbox.root == root) {
        let _ = inbox.tx.send(Msg::SetIgnored(taken, ignored.clone()));
    }
}

/// Keeps the watcher alive; dropping it stops watching (stops the FSEvents stream, joins the
/// coalescer thread). Never drop it from inside its own emit callback.
pub struct RepoWatcher {
    /// Key of this watcher's entry in [`INBOXES`].
    inbox: u64,
    watcher: Option<RecommendedWatcher>,
    tx: Sender<Msg>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for RepoWatcher {
    fn drop(&mut self) {
        inboxes().retain(|inbox| inbox.id != self.inbox);
        // Stop the stream first (joins its run-loop thread) so no event arrives after Stop.
        drop(self.watcher.take());
        let _ = self.tx.send(Msg::Stop);
        if let Some(t) = self.thread.take() {
            if t.thread().id() != std::thread::current().id() {
                let _ = t.join();
            }
        }
    }
}

/// Watch the worktree and git dirs of `info`; emits `repo-changed` ([`RepoChanged`]) once per
/// burst of changes. Blocks for a few ms while the FSEvents stream starts: call it off the main
/// thread.
pub fn start(app: AppHandle, info: &RepoInfo) -> AppResult<RepoWatcher> {
    // FSEvents reports real paths (/private/tmp, resolved symlinks, on-disk case).
    let roots = RepoInfo {
        path: canonical(&info.path)?,
        git_dir: canonical(&info.git_dir)?,
        common_dir: canonical(&info.common_dir)?,
        ..info.clone()
    };
    let (tx, rx) = mpsc::channel();
    let fs_tx = tx.clone();
    let mut watcher = notify::recommended_watcher(move |res: notify::Result<Event>| {
        let _ = fs_tx.send(Msg::Fs(res));
    })
    .map_err(watch_error)?;
    {
        // Every watch()/unwatch() restarts the stream: add all roots in one commit.
        let mut paths = watcher.paths_mut();
        for dir in watch_roots(&roots) {
            paths.add(dir, RecursiveMode::Recursive).map_err(watch_error)?;
        }
        paths.commit().map_err(watch_error)?;
    }
    // A bare repo has no work tree, hence no status to take ignored paths from.
    let root = (!roots.is_bare).then(|| PathBuf::from(&roots.path));
    let repo = info.path.clone();
    let thread = std::thread::Builder::new().name("repo-watch".into()).spawn(move || {
        coalesce(&rx, &roots, |bits| {
            let _ = app.emit("repo-changed", RepoChanged { repo: repo.clone(), kinds: kind_names(bits) });
        })
    })?;
    let inbox = NEXT_INBOX.fetch_add(1, Ordering::Relaxed);
    if let Some(root) = root {
        inboxes().push(Inbox { id: inbox, root, tx: tx.clone() });
    }
    Ok(RepoWatcher { inbox, watcher: Some(watcher), tx, thread: Some(thread) })
}

fn canonical(path: &str) -> AppResult<String> {
    Ok(std::fs::canonicalize(path)?.to_string_lossy().into_owned())
}

fn watch_error(e: notify::Error) -> AppError {
    AppError::msg(format!("file watcher: {e}"))
}

/// Directories to watch recursively: the worktree, plus the common dir (and the git dir) unless
/// already inside a watched one (linked worktree, bare repo, separate git dir, submodule).
fn watch_roots(info: &RepoInfo) -> Vec<&Path> {
    let workdir = (!info.is_bare).then(|| Path::new(&info.path));
    let mut roots: Vec<&Path> = Vec::with_capacity(3);
    for dir in workdir.into_iter().chain([Path::new(&info.common_dir), Path::new(&info.git_dir)]) {
        if !roots.iter().any(|r| dir.starts_with(r)) {
            roots.push(dir);
        }
    }
    roots
}

/// Accumulate classified kinds and call `emit` once the burst is quiet (80 ms git-only, 250 ms
/// with worktree changes) or [`MAX_DELAY`] after it started. Returns on Stop / disconnect.
fn coalesce(rx: &Receiver<Msg>, roots: &RepoInfo, mut emit: impl FnMut(u16)) {
    let mut pending = 0u16;
    let mut first: Option<Instant> = None;
    let mut last = Instant::now();
    // Ignored paths of the newest status; `ignored_since` is when that status started, or when they
    // were last found stale. Lists from statuses that started earlier are refused.
    let mut ignored = Arc::<IgnoredPaths>::default();
    let mut ignored_since: Option<Instant> = None;
    loop {
        let msg = match first {
            None => rx.recv().map_err(|_| RecvTimeoutError::Disconnected),
            Some(start) => {
                let quiet = if pending & SLOW_KINDS != 0 { QUIET_WORKTREE } else { QUIET_GIT };
                let deadline = (last + quiet).min(start + MAX_DELAY);
                match deadline.checked_duration_since(Instant::now()) {
                    Some(wait) if !wait.is_zero() => rx.recv_timeout(wait),
                    // Also covers a channel flooded faster than it drains.
                    _ => Err(RecvTimeoutError::Timeout),
                }
            }
        };
        match msg {
            Ok(Msg::Fs(res)) => {
                // Changes below ignored paths classify as 0: they neither start nor extend a burst.
                let bits = res.map_or(kind::RESCAN, |ev| classify_event(&ev, roots, &ignored));
                if bits & STALE_IGNORED != 0 {
                    // Filter nothing until a status started from now on lists them again (every
                    // STALE_IGNORED kind makes the frontend refresh the status).
                    ignored = Arc::default();
                    ignored_since = Some(Instant::now());
                }
                if bits != 0 {
                    pending |= bits;
                    last = Instant::now();
                    first.get_or_insert(last);
                }
            }
            Ok(Msg::SetIgnored(taken, paths)) => {
                if ignored_since.is_none_or(|since| since <= taken) {
                    ignored = paths;
                    ignored_since = Some(taken);
                }
            }
            Ok(Msg::Stop) | Err(RecvTimeoutError::Disconnected) => break,
            Err(RecvTimeoutError::Timeout) => {
                emit(pending);
                pending = 0;
                first = None;
            }
        }
    }
}

/// `repo-changed` kind names for a bit set: sorted, deduplicated; RESCAN expands to all of them.
fn kind_names(bits: u16) -> Vec<String> {
    NAMES.iter().filter(|(_, mask)| bits & (mask | kind::RESCAN) != 0).map(|(name, _)| name.to_string()).collect()
}

/// Kinds touched by one notify event (0 = ignore).
fn classify_event(ev: &Event, info: &RepoInfo, ignored: &IgnoredPaths) -> u16 {
    if ev.need_rescan() {
        return kind::RESCAN; // FSEvents MustScanSubDirs: the kernel or user space dropped events
    }
    if matches!(ev.kind, EventKind::Access(_) | EventKind::Other) {
        return 0;
    }
    ev.paths.iter().fold(0, |acc, path| acc | classify(path, info, ignored))
}

/// Map a changed path to change kinds (bit set of `kind::*`), 0 = ignore. `ignored` holds the
/// worktree's gitignored paths (relative to `info.path`).
/// Pure: `info` paths must already be canonical (as FSEvents reports them).
pub fn classify(path: &Path, info: &RepoInfo, ignored: &IgnoredPaths) -> u16 {
    let git_dir = Path::new(&info.git_dir);
    let common_dir = Path::new(&info.common_dir);
    let workdir = (!info.is_bare).then(|| Path::new(&info.path));
    let in_workdir = workdir.and_then(|wd| path.strip_prefix(wd).ok());
    // The most specific root wins. Usually the git dir is inside the worktree, but a worktree can
    // also live inside the common dir (`git worktree add <name>` run in a bare repo).
    if let Some(rel) = in_workdir {
        if workdir.is_some_and(|wd| wd.starts_with(common_dir)) {
            return worktree_file(rel, ignored);
        }
    }
    if let Ok(rel) = path.strip_prefix(git_dir) {
        if let Some(bits) = git_dir_file(rel) {
            return bits;
        }
        // A linked worktree's own reftable stack holds its HEAD. Its other private files fall
        // through to the common dir rules (`worktrees/<id>/{gitdir,locked}` → WORKTREES, else 0).
        if git_dir != common_dir && rel.starts_with("reftable") {
            return kind::HEAD;
        }
    }
    if let Ok(rel) = path.strip_prefix(common_dir) {
        return common_dir_file(rel);
    }
    in_workdir.map_or(0, |rel| worktree_file(rel, ignored))
}

/// Files of the worktree's own git dir; None = not per-worktree (look in the common dir).
fn git_dir_file(rel: &Path) -> Option<u16> {
    use kind::*;
    let s = rel.to_str()?;
    if s.ends_with(".lock") {
        return Some(0); // the final rename onto the real name follows
    }
    Some(match s {
        "index" => INDEX,
        "HEAD" | "logs/HEAD" => HEAD,
        "config.worktree" => CONFIG,
        "MERGE_HEAD" | "MERGE_MSG" | "CHERRY_PICK_HEAD" | "REVERT_HEAD" | "REBASE_HEAD" | "BISECT_LOG"
        | "BISECT_START" => STATE,
        _ if ["rebase-merge", "rebase-apply", "sequencer"].iter().any(|d| rel.starts_with(d)) => STATE,
        _ => return None,
    })
}

/// Files of the shared git dir.
fn common_dir_file(rel: &Path) -> u16 {
    use kind::*;
    let Some(s) = rel.to_str() else { return 0 };
    if s.ends_with(".lock") {
        return 0;
    }
    if let Some(rest) = s.strip_prefix("worktrees/") {
        return match rest.split_once('/') {
            None | Some((_, "HEAD" | "gitdir" | "locked")) => WORKTREES,
            Some((_, f)) if f.starts_with("reftable/") => WORKTREES,
            _ => 0,
        };
    }
    match s {
        "" => RESCAN, // the git dir itself was replaced
        "packed-refs" | "shallow" => REFS,
        "refs/stash" | "logs/refs/stash" => STASH, // `stash drop stash@{n}` rewrites only the reflog
        "config" | "config.worktree" => CONFIG,
        "info/exclude" => WORKTREE | IGNORES,
        "HEAD" => WORKTREES, // reached only from a linked worktree: the main worktree's HEAD moved
        "worktrees" => WORKTREES,
        _ if rel.starts_with("refs") => REFS,
        _ if rel.starts_with("reftable") => REFS | STASH | HEAD,
        // A submodule's HEAD/index moved: the superproject's status changes.
        _ if s.starts_with("modules/") && (s.ends_with("/HEAD") || s.ends_with("/index")) => WORKTREE,
        // objects/**, other logs, FETCH_HEAD, ORIG_HEAD, AUTO_MERGE, COMMIT_EDITMSG,
        // fsmonitor--daemon/** (a cookie per git command), hooks/**, lfs/**, gc.pid, ...
        _ => 0,
    }
}

/// Paths inside the working tree.
fn worktree_file(rel: &Path, ignored: &IgnoredPaths) -> u16 {
    if rel.components().any(|c| c.as_os_str() == ".git") {
        return 0; // nested repos, submodule gitlinks, a linked worktree's `.git` file
    }
    let name = rel.file_name();
    if name.is_some_and(|n| n == ".DS_Store") || ignored.covers(rel) {
        return 0; // e.g. build output, node_modules, logs
    }
    if name.is_some_and(|n| n == ".gitignore") {
        kind::WORKTREE | kind::IGNORES
    } else {
        kind::WORKTREE
    }
}

#[cfg(test)]
mod tests {
    use super::kind::*;
    use super::*;
    use notify::event::{AccessKind, CreateKind, Flag, ModifyKind, RemoveKind, RenameMode};

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
    fn main_repo() -> RepoInfo {
        info("/r", "/r/.git", "/r/.git", false)
    }
    fn linked() -> RepoInfo {
        info("/w", "/r/.git/worktrees/w", "/r/.git", false)
    }
    /// Classify with no ignored paths known (before the first status).
    fn c(info: &RepoInfo, path: &str) -> u16 {
        classify(Path::new(path), info, &IgnoredPaths::default())
    }
    fn ignored_set(records: &[&str]) -> IgnoredPaths {
        let mut set = IgnoredPaths::default();
        records.iter().for_each(|r| set.insert(r.as_bytes()));
        set
    }
    fn ignored(records: &[&str]) -> Arc<IgnoredPaths> {
        Arc::new(ignored_set(records))
    }

    #[test]
    fn main_worktree_files() {
        let p = main_repo();
        assert_eq!(c(&p, "/r"), WORKTREE);
        assert_eq!(c(&p, "/r/src/a.rs"), WORKTREE);
        assert_eq!(c(&p, "/r/.gitignore"), WORKTREE | IGNORES);
        assert_eq!(c(&p, "/r/src/.gitignore"), WORKTREE | IGNORES);
        assert_eq!(c(&p, "/r/Cargo.lock"), WORKTREE);
        assert_eq!(c(&p, "/r/node_modules/x/y.js"), WORKTREE); // ignored paths not known yet
        assert_eq!(c(&p, "/r/.DS_Store"), 0);
        assert_eq!(c(&p, "/r/src/.DS_Store"), 0);
        assert_eq!(c(&p, "/r/vendor/lib/.git"), 0);
        assert_eq!(c(&p, "/r/vendor/lib/.git/HEAD"), 0);
        assert_eq!(c(&p, "/r/vendor/lib/.git/refs/heads/main"), 0);
        assert_eq!(c(&p, "/r/sub/.git"), 0);
        assert_eq!(c(&p, "/r/sub/file.txt"), WORKTREE);
        assert_eq!(c(&p, "/elsewhere/a.rs"), 0);
        assert_eq!(c(&p, "/rr/a.rs"), 0);
    }

    #[test]
    fn main_git_dir() {
        let p = main_repo();
        assert_eq!(c(&p, "/r/.git"), RESCAN);
        assert_eq!(c(&p, "/r/.git/index"), INDEX);
        assert_eq!(c(&p, "/r/.git/index.lock"), 0);
        assert_eq!(c(&p, "/r/.git/index.stash.123"), 0);
        assert_eq!(c(&p, "/r/.git/HEAD"), HEAD);
        assert_eq!(c(&p, "/r/.git/HEAD.lock"), 0);
        assert_eq!(c(&p, "/r/.git/logs/HEAD"), HEAD);
        assert_eq!(c(&p, "/r/.git/refs/heads/x"), REFS);
        assert_eq!(c(&p, "/r/.git/refs/heads/feat/x"), REFS);
        assert_eq!(c(&p, "/r/.git/refs/heads/x.lock"), 0);
        assert_eq!(c(&p, "/r/.git/refs/tags/v1"), REFS);
        assert_eq!(c(&p, "/r/.git/refs/remotes/origin/main"), REFS);
        assert_eq!(c(&p, "/r/.git/refs/heads"), REFS);
        assert_eq!(c(&p, "/r/.git/packed-refs"), REFS);
        assert_eq!(c(&p, "/r/.git/packed-refs.lock"), 0);
        assert_eq!(c(&p, "/r/.git/shallow"), REFS);
        assert_eq!(c(&p, "/r/.git/refs/stash"), STASH);
        assert_eq!(c(&p, "/r/.git/logs/refs/stash"), STASH);
        assert_eq!(c(&p, "/r/.git/logs/refs/heads/main"), 0);
        assert_eq!(c(&p, "/r/.git/config"), CONFIG);
        assert_eq!(c(&p, "/r/.git/config.lock"), 0);
        assert_eq!(c(&p, "/r/.git/config.worktree"), CONFIG);
        assert_eq!(c(&p, "/r/.git/info/exclude"), WORKTREE | IGNORES);
        assert_eq!(c(&p, "/r/.git/objects/ab/cdef0123"), 0);
        assert_eq!(c(&p, "/r/.git/objects/pack/pack-1.pack"), 0);
        assert_eq!(c(&p, "/r/.git/objects/info/commit-graph"), 0);
        assert_eq!(c(&p, "/r/.git/fsmonitor--daemon/cookies/123-4"), 0);
        assert_eq!(c(&p, "/r/.git/fsmonitor--daemon.ipc"), 0);
        assert_eq!(c(&p, "/r/.git/FETCH_HEAD"), 0);
        assert_eq!(c(&p, "/r/.git/ORIG_HEAD"), 0);
        assert_eq!(c(&p, "/r/.git/AUTO_MERGE"), 0);
        assert_eq!(c(&p, "/r/.git/COMMIT_EDITMSG"), 0);
        assert_eq!(c(&p, "/r/.git/hooks/pre-commit"), 0);
        assert_eq!(c(&p, "/r/.git/lfs/tmp/x"), 0);
        assert_eq!(c(&p, "/r/.git/gc.pid"), 0);
        assert_eq!(c(&p, "/r/.git/.DS_Store"), 0);
    }

    #[test]
    fn main_state_files() {
        let p = main_repo();
        let files = ["MERGE_HEAD", "MERGE_MSG", "CHERRY_PICK_HEAD", "REVERT_HEAD", "REBASE_HEAD", "BISECT_LOG", "BISECT_START"];
        for f in files {
            assert_eq!(c(&p, &format!("/r/.git/{f}")), STATE, "{f}");
        }
        assert_eq!(c(&p, "/r/.git/rebase-merge"), STATE);
        assert_eq!(c(&p, "/r/.git/rebase-merge/done"), STATE);
        assert_eq!(c(&p, "/r/.git/rebase-merge/msgnum"), STATE);
        assert_eq!(c(&p, "/r/.git/rebase-apply/next"), STATE);
        assert_eq!(c(&p, "/r/.git/sequencer/todo"), STATE);
        assert_eq!(c(&p, "/r/.git/rebase-merge-x"), 0);
        assert_eq!(c(&p, "/r/.git/MERGE_MODE"), 0);
    }

    #[test]
    fn main_view_of_worktrees_and_submodules() {
        let p = main_repo();
        assert_eq!(c(&p, "/r/.git/worktrees"), WORKTREES);
        assert_eq!(c(&p, "/r/.git/worktrees/w"), WORKTREES);
        assert_eq!(c(&p, "/r/.git/worktrees/w/HEAD"), WORKTREES);
        assert_eq!(c(&p, "/r/.git/worktrees/w/gitdir"), WORKTREES);
        assert_eq!(c(&p, "/r/.git/worktrees/w/locked"), WORKTREES);
        assert_eq!(c(&p, "/r/.git/worktrees/w/HEAD.lock"), 0);
        assert_eq!(c(&p, "/r/.git/worktrees/w/index"), 0);
        assert_eq!(c(&p, "/r/.git/worktrees/w/logs/HEAD"), 0);
        assert_eq!(c(&p, "/r/.git/worktrees/w/ORIG_HEAD"), 0);
        assert_eq!(c(&p, "/r/.git/modules/sub/HEAD"), WORKTREE);
        assert_eq!(c(&p, "/r/.git/modules/sub/index"), WORKTREE);
        assert_eq!(c(&p, "/r/.git/modules/libs/a/index"), WORKTREE);
        assert_eq!(c(&p, "/r/.git/modules/sub/index.lock"), 0);
        assert_eq!(c(&p, "/r/.git/modules/sub/objects/ab/cd"), 0);
        assert_eq!(c(&p, "/r/.git/modules/sub/refs/heads/main"), 0);
    }

    #[test]
    fn linked_worktree() {
        let p = linked();
        assert_eq!(c(&p, "/w"), WORKTREE);
        assert_eq!(c(&p, "/w/src/a.rs"), WORKTREE);
        assert_eq!(c(&p, "/w/.git"), 0);
        assert_eq!(c(&p, "/w/.DS_Store"), 0);
        assert_eq!(c(&p, "/r/.git/worktrees/w/index"), INDEX);
        assert_eq!(c(&p, "/r/.git/worktrees/w/index.lock"), 0);
        assert_eq!(c(&p, "/r/.git/worktrees/w/HEAD"), HEAD);
        assert_eq!(c(&p, "/r/.git/worktrees/w/logs/HEAD"), HEAD);
        assert_eq!(c(&p, "/r/.git/worktrees/w/config.worktree"), CONFIG);
        assert_eq!(c(&p, "/r/.git/worktrees/w/MERGE_HEAD"), STATE);
        assert_eq!(c(&p, "/r/.git/worktrees/w/rebase-merge/msgnum"), STATE);
        assert_eq!(c(&p, "/r/.git/worktrees/w/ORIG_HEAD"), 0);
        assert_eq!(c(&p, "/r/.git/worktrees/w/COMMIT_EDITMSG"), 0);
        assert_eq!(c(&p, "/r/.git/worktrees/w/commondir"), 0);
        // `git worktree lock/unlock/move/repair` of this worktree changes the worktree list.
        assert_eq!(c(&p, "/r/.git/worktrees/w/gitdir"), WORKTREES);
        assert_eq!(c(&p, "/r/.git/worktrees/w/locked"), WORKTREES);
        assert_eq!(c(&p, "/r/.git/worktrees/w/refs/bisect/bad"), 0);
        assert_eq!(c(&p, "/r/.git/worktrees/w/fsmonitor--daemon/cookies/1-2"), 0);
        // Its own admin dir appearing/disappearing is a worktree-list change.
        assert_eq!(c(&p, "/r/.git/worktrees/w"), WORKTREES);
        assert_eq!(c(&p, "/r/.git/worktrees/other/HEAD"), WORKTREES);
        assert_eq!(c(&p, "/r/.git/worktrees/other/index"), 0);
        // Shared dir: refs and config are ours too; the main worktree's private files are not.
        assert_eq!(c(&p, "/r/.git/HEAD"), WORKTREES);
        assert_eq!(c(&p, "/r/.git/refs/heads/main"), REFS);
        assert_eq!(c(&p, "/r/.git/packed-refs"), REFS);
        assert_eq!(c(&p, "/r/.git/refs/stash"), STASH);
        assert_eq!(c(&p, "/r/.git/logs/refs/stash"), STASH);
        assert_eq!(c(&p, "/r/.git/config"), CONFIG);
        assert_eq!(c(&p, "/r/.git/info/exclude"), WORKTREE | IGNORES);
        assert_eq!(c(&p, "/r/.git/index"), 0);
        assert_eq!(c(&p, "/r/.git/logs/HEAD"), 0);
        assert_eq!(c(&p, "/r/.git/MERGE_HEAD"), 0);
        assert_eq!(c(&p, "/r/.git/objects/ab/cd"), 0);
        assert_eq!(c(&p, "/r/src/a.rs"), 0);
    }

    #[test]
    fn bare_and_separate_git_dirs() {
        let b = info("/b.git", "/b.git", "/b.git", true);
        assert_eq!(c(&b, "/b.git"), RESCAN);
        assert_eq!(c(&b, "/b.git/HEAD"), HEAD);
        assert_eq!(c(&b, "/b.git/refs/tags/v1"), REFS);
        assert_eq!(c(&b, "/b.git/packed-refs"), REFS);
        assert_eq!(c(&b, "/b.git/config"), CONFIG);
        assert_eq!(c(&b, "/b.git/objects/ab/cd"), 0);
        assert_eq!(c(&b, "/elsewhere/x"), 0);

        let s = info("/r", "/x/repo.git", "/x/repo.git", false);
        assert_eq!(c(&s, "/r/.git"), 0);
        assert_eq!(c(&s, "/r/a.txt"), WORKTREE);
        assert_eq!(c(&s, "/x/repo.git/index"), INDEX);
        assert_eq!(c(&s, "/x/repo.git/refs/heads/main"), REFS);

        let m = info("/r/sub", "/r/.git/modules/sub", "/r/.git/modules/sub", false);
        assert_eq!(c(&m, "/r/sub/a.txt"), WORKTREE);
        assert_eq!(c(&m, "/r/sub/.git"), 0);
        assert_eq!(c(&m, "/r/.git/modules/sub/index"), INDEX);
        assert_eq!(c(&m, "/r/.git/modules/sub/HEAD"), HEAD);
        assert_eq!(c(&m, "/r/.git/index"), 0);
        assert_eq!(c(&m, "/r/other.txt"), 0);
    }

    #[test]
    fn worktree_inside_bare_repo() {
        // `git clone --bare url b.git && cd b.git && git worktree add main` (verified with git 2.48).
        let n = info("/b.git/main", "/b.git/worktrees/main", "/b.git", false);
        assert_eq!(c(&n, "/b.git/main"), WORKTREE);
        assert_eq!(c(&n, "/b.git/main/src/a.rs"), WORKTREE);
        assert_eq!(c(&n, "/b.git/main/refs/a.txt"), WORKTREE);
        assert_eq!(c(&n, "/b.git/main/config"), WORKTREE);
        assert_eq!(c(&n, "/b.git/main/.git"), 0);
        assert_eq!(c(&n, "/b.git/main/.DS_Store"), 0);
        assert_eq!(c(&n, "/b.git/worktrees/main/index"), INDEX);
        assert_eq!(c(&n, "/b.git/worktrees/main/HEAD"), HEAD);
        assert_eq!(c(&n, "/b.git/worktrees/main/locked"), WORKTREES);
        assert_eq!(c(&n, "/b.git/worktrees/main/ORIG_HEAD"), 0);
        assert_eq!(c(&n, "/b.git/refs/heads/main"), REFS);
        assert_eq!(c(&n, "/b.git/config"), CONFIG);
        assert_eq!(c(&n, "/b.git/HEAD"), WORKTREES);
        assert_eq!(c(&n, "/b.git/objects/ab/cd"), 0);
        // A sibling worktree nested in the same bare repo.
        assert_eq!(c(&n, "/b.git/feat/a.rs"), 0);
        assert_eq!(c(&n, "/b.git/worktrees/feat/HEAD"), WORKTREES);
        // The bare repo itself ignores its nested worktrees' files.
        let b = info("/b.git", "/b.git", "/b.git", true);
        assert_eq!(c(&b, "/b.git/main/src/a.rs"), 0);
        assert_eq!(c(&b, "/b.git/worktrees/main/HEAD"), WORKTREES);
    }

    #[test]
    fn reftable() {
        let p = main_repo();
        assert_eq!(c(&p, "/r/.git/reftable/tables.list"), REFS | STASH | HEAD);
        assert_eq!(c(&p, "/r/.git/reftable/0x000000000001-0x000000000004-97d7f599.ref"), REFS | STASH | HEAD);
        assert_eq!(c(&p, "/r/.git/reftable/tables.list.lock"), 0);
        assert_eq!(c(&p, "/r/.git/worktrees/w/reftable/tables.list"), WORKTREES);
        let l = linked();
        assert_eq!(c(&l, "/r/.git/worktrees/w/reftable/tables.list"), HEAD);
        assert_eq!(c(&l, "/r/.git/worktrees/w/reftable/tables.list.lock"), 0);
        assert_eq!(c(&l, "/r/.git/reftable/tables.list"), REFS | STASH | HEAD);
    }

    #[test]
    fn events() {
        let p = main_repo();
        let none = IgnoredPaths::default();
        let ev = |kind: EventKind, paths: &[&str]| {
            paths.iter().fold(Event::new(kind), |ev, path| ev.add_path(PathBuf::from(path)))
        };
        let rescan = Event::new(EventKind::Other).set_flag(Flag::Rescan).add_path("/r".into());
        assert_eq!(classify_event(&rescan, &p, &none), RESCAN);
        assert_eq!(classify_event(&ev(EventKind::Other, &["/r/a.txt"]), &p, &none), 0);
        assert_eq!(classify_event(&ev(EventKind::Access(AccessKind::Any), &["/r/a.txt"]), &p, &none), 0);
        let rename = EventKind::Modify(ModifyKind::Name(RenameMode::Any));
        assert_eq!(classify_event(&ev(rename, &["/r/.git/index.lock", "/r/.git/index"]), &p, &none), INDEX);
        assert_eq!(classify_event(&ev(rename, &["/r/a.txt", "/r/.git/HEAD"]), &p, &none), WORKTREE | HEAD);
        assert_eq!(classify_event(&ev(EventKind::Create(CreateKind::File), &["/r/.git/objects/ab/cd"]), &p, &none), 0);
        assert_eq!(classify_event(&ev(EventKind::Remove(RemoveKind::File), &["/r/b.txt"]), &p, &none), WORKTREE);
        assert_eq!(classify_event(&ev(EventKind::Any, &[]), &p, &none), 0);
        // Moving a file out of an ignored directory is a change; moving it within is not.
        let target = ignored(&["target/"]);
        assert_eq!(classify_event(&ev(rename, &["/r/target/a.o", "/r/target/b.o"]), &p, &target), 0);
        assert_eq!(classify_event(&ev(rename, &["/r/target/a.o", "/r/a.o"]), &p, &target), WORKTREE);
    }

    /// `! ` records of `git status --porcelain=v2 -z --ignored=matching -uall` in a repo ignoring
    /// `node_modules/`, `target`, `*.log`, `build/` (holding the tracked `build/keep.txt`), `dist/*`
    /// (except `dist/.gitkeep`), `empty-ign/`, `nested/` (a nested repo), and `src/.gitignore` via
    /// `info/exclude` (git 2.48.1).
    const IGNORED: [&str; 11] = [
        "app.log",
        "build/.gitignore",
        "build/gen.o",
        "build/sub/",
        "dist/main.123.js",
        "empty-ign/",
        "nested/",
        "node_modules/",
        "src/.gitignore",
        "src/b.log",
        "target/",
    ];

    #[test]
    fn ignored_paths() {
        let p = main_repo();
        let set = ignored_set(&IGNORED);
        let c = |path: &str| classify(Path::new(path), &p, &set);
        assert_eq!(c("/r/node_modules/x.js"), 0);
        assert_eq!(c("/r/node_modules/a/b/c.js"), 0);
        assert_eq!(c("/r/node_modules/a/.gitignore"), 0); // git never reads it
        assert_eq!(c("/r/target/debug/build/x-1/out/gen.rs"), 0);
        assert_eq!(c("/r/build/gen.o"), 0);
        assert_eq!(c("/r/build/sub/x.o"), 0);
        assert_eq!(c("/r/empty-ign/new"), 0);
        assert_eq!(c("/r/nested/f"), 0);
        assert_eq!(c("/r/src/b.log"), 0);
        assert_eq!(c("/r/app.log"), 0);
        assert_eq!(c("/r/dist/main.123.js"), 0);
        // The ignored directory itself: a `name/` pattern does not ignore a file put in its place.
        assert_eq!(c("/r/node_modules"), WORKTREE);
        assert_eq!(c("/r/target"), WORKTREE);
        // Tracked, or not listed (yet: the status that this change triggers lists new ignored files).
        assert_eq!(c("/r/build/keep.txt"), WORKTREE);
        assert_eq!(c("/r/build/new.o"), WORKTREE);
        assert_eq!(c("/r/dist/main.456.js"), WORKTREE);
        assert_eq!(c("/r/dist/.gitkeep"), WORKTREE);
        assert_eq!(c("/r/app.log.1"), WORKTREE);
        assert_eq!(c("/r/app.log/x"), WORKTREE); // a directory replaced the ignored file
        assert_eq!(c("/r/node_modules2/x.js"), WORKTREE);
        assert_eq!(c("/r/src/a.rs"), WORKTREE);
        // git honors an ignored `.gitignore` in a directory it walks.
        assert_eq!(c("/r/src/.gitignore"), WORKTREE | IGNORES);
        assert_eq!(c("/r/build/.gitignore"), WORKTREE | IGNORES);
        assert_eq!(c("/r/.gitignore"), WORKTREE | IGNORES);
        // Never applied to the git dir.
        assert_eq!(c("/r/.git/index"), INDEX);
        assert_eq!(c("/r/.git/info/exclude"), WORKTREE | IGNORES);
        // Empty records match nothing (`/` would otherwise cover the whole worktree).
        assert_eq!(classify(Path::new("/r/a"), &p, &ignored_set(&["", "/"])), WORKTREE);
        // Relative to each worktree's own root.
        let set = ignored_set(&["node_modules/"]);
        assert_eq!(classify(Path::new("/w/node_modules/x"), &linked(), &set), 0);
        assert_eq!(classify(Path::new("/w/src/a.rs"), &linked(), &set), WORKTREE);
        assert_eq!(classify(Path::new("/r/.git/worktrees/w/index"), &linked(), &set), INDEX);
        let nested = info("/b.git/main", "/b.git/worktrees/main", "/b.git", false);
        assert_eq!(classify(Path::new("/b.git/main/node_modules/x"), &nested, &set), 0);
        assert_eq!(classify(Path::new("/b.git/main/src/a.rs"), &nested, &set), WORKTREE);
    }

    #[test]
    fn ignored_files_are_capped_but_dirs_kept() {
        let mut set = IgnoredPaths::default();
        for i in 0..=MAX_IGNORED_FILES {
            set.insert(format!("obj/{i}.o").as_bytes());
        }
        set.insert(b"node_modules/");
        assert_eq!(set.files.len(), MAX_IGNORED_FILES);
        assert!(set.covers(Path::new("obj/0.o")));
        assert!(!set.covers(Path::new(&format!("obj/{MAX_IGNORED_FILES}.o")))); // not filtered: safe
        assert!(set.covers(Path::new("node_modules/x")));
    }

    /// Queue `msgs`, run a coalescer over them and collect what it emits until every bit of `want`
    /// arrived (a stalled test thread may split the burst) or 2 s passed.
    fn coalesced(msgs: Vec<Msg>, want: u16) -> u16 {
        let (tx, rx) = mpsc::channel();
        msgs.into_iter().for_each(|m| tx.send(m).unwrap());
        let (out_tx, out_rx) = mpsc::channel();
        let thread = std::thread::spawn(move || coalesce(&rx, &main_repo(), |bits| out_tx.send(bits).unwrap()));
        let mut got = 0;
        while got & want != want {
            match out_rx.recv_timeout(Duration::from_secs(2)) {
                Ok(bits) => got |= bits,
                Err(_) => break,
            }
        }
        drop(tx);
        thread.join().unwrap();
        got
    }
    fn modified(path: &str) -> Msg {
        Msg::Fs(Ok(Event::new(EventKind::Modify(ModifyKind::Any)).add_path(path.into())))
    }

    #[test]
    fn coalescer_drops_changes_below_ignored_paths() {
        let t0 = Instant::now();
        let msgs = vec![
            Msg::SetIgnored(t0 + Duration::from_millis(2), ignored(&["target/"])),
            Msg::SetIgnored(t0, ignored(&[])), // from an older status: refused
            modified("/r/target/debug/a.o"),
            modified("/r/.git/HEAD"),
        ];
        assert_eq!(coalesced(msgs, HEAD), HEAD);
    }

    #[test]
    fn coalescer_stops_filtering_when_the_index_changes() {
        let t0 = Instant::now();
        let target = ignored(&["target/"]);
        let msgs = vec![
            Msg::SetIgnored(t0, target.clone()),
            modified("/r/.git/index"),
            Msg::SetIgnored(t0, target.clone()), // from a status that started before: refused
            modified("/r/target/a.o"),
        ];
        assert_eq!(coalesced(msgs, INDEX | WORKTREE), INDEX | WORKTREE);
        // A list from a status that started afterwards is taken again.
        let msgs = vec![
            Msg::SetIgnored(t0, target.clone()),
            modified("/r/.git/index"),
            Msg::SetIgnored(Instant::now() + Duration::from_secs(60), target),
            modified("/r/target/a.o"),
        ];
        assert_eq!(coalesced(msgs, INDEX), INDEX);
    }

    #[test]
    fn ignored_paths_reach_the_watchers_of_their_worktree() {
        // Any existing directory works: the inbox is registered by hand (`start` needs an app).
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"));
        let root = std::fs::canonicalize(dir).unwrap();
        let (tx, rx) = mpsc::channel();
        inboxes().push(Inbox { id: u64::MAX, root: root.clone(), tx });
        assert_eq!(watched_root(dir), Some(root.clone()));
        assert_eq!(watched_root(&dir.join("src/..")), Some(root.clone()));
        assert_eq!(watched_root(&dir.join("src")), None);
        let taken = Instant::now();
        share_ignored(&root.join("src"), taken, IgnoredPaths::default());
        share_ignored(&root, taken, ignored_set(&["target/"]));
        match rx.try_recv() {
            Ok(Msg::SetIgnored(at, paths)) => {
                assert_eq!(at, taken);
                assert!(paths.covers(Path::new("target/debug/a.o")));
            }
            _ => panic!("expected the ignored paths"),
        }
        assert!(rx.try_recv().is_err());
        inboxes().retain(|inbox| inbox.id != u64::MAX);
        assert_eq!(watched_root(dir), None);
    }

    #[test]
    fn names() {
        assert_eq!(kind_names(WORKTREE), ["worktree"]);
        assert_eq!(kind_names(WORKTREE | IGNORES), ["worktree"]);
        assert_eq!(kind_names(INDEX | HEAD | REFS), ["head", "index", "refs"]);
        assert_eq!(kind_names(REFS | WORKTREES), ["refs"]);
        assert_eq!(kind_names(WORKTREES), ["refs"]);
        assert_eq!(kind_names(STASH | STATE | CONFIG), ["config", "stash", "state"]);
        assert_eq!(kind_names(RESCAN), ["config", "head", "index", "refs", "stash", "state", "worktree"]);
        assert_eq!(kind_names(RESCAN | INDEX), kind_names(RESCAN));
        assert!(kind_names(0).is_empty());
    }

    #[test]
    fn roots() {
        let paths = |i: &RepoInfo| watch_roots(i).iter().map(|p| p.to_string_lossy().into_owned()).collect::<Vec<_>>();
        assert_eq!(paths(&main_repo()), ["/r"]);
        assert_eq!(paths(&linked()), ["/w", "/r/.git"]);
        assert_eq!(paths(&info("/b.git", "/b.git", "/b.git", true)), ["/b.git"]);
        assert_eq!(paths(&info("/r", "/x/repo.git", "/x/repo.git", false)), ["/r", "/x/repo.git"]);
        let submodule = info("/r/sub", "/r/.git/modules/sub", "/r/.git/modules/sub", false);
        assert_eq!(paths(&submodule), ["/r/sub", "/r/.git/modules/sub"]);
        assert_eq!(paths(&info("/r/wt/a", "/r/.git/worktrees/a", "/r/.git", false)), ["/r/wt/a", "/r/.git"]);
        // Overlapping roots are fine: one FSEvents stream reports each change once (verified).
        assert_eq!(paths(&info("/b.git/main", "/b.git/worktrees/main", "/b.git", false)), ["/b.git/main", "/b.git"]);
    }
}
