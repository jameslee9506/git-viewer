//! The write operations the viewer offers: stage / unstage, commit, branch and tag creation and
//! deletion (here and on a remote), checkout, reset, cherry-pick / revert (with continue and abort),
//! stash and the network trio fetch / pull / push, plus the background fetch that keeps the remote
//! branches current (`fetch_background`: the one write nobody clicks for). Every other command stays
//! read-only. Each function runs one git process (plus a check or two for stashes, names, where a ref
//! to delete is and how a cherry-pick ended; the background fetch one per remote) through `cmd::Git`,
//! so the usual env and config overrides apply (hooks, LFS filters and credential helpers resolve on
//! the login-shell PATH, see `cmd::path_env`); the index lock is a required lock, which
//! `--no-optional-locks` leaves alone.

use crate::error::{AppError, AppResult};
use crate::git::cmd::{lossy, split_records, Git, GitOutput};
use crate::git::repo;
use crate::model::{CheckoutTarget, OpResult, PickOp, PickOutcome, RemoteFetch, RepoInfo, ResetMode};
use std::collections::HashSet;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// Network operations are killed after this long.
const NET_TIMEOUT: Duration = Duration::from_secs(600);

/// The background fetch of one remote is stopped once git has reported nothing for this long: a
/// connection that hangs (the machine slept through it) must not hold back the next fetch. There is
/// no limit on its total time: nobody waits for it, and one stopped half-way would start over the
/// next time, leaving its partial pack behind each time.
const BACKGROUND_IDLE: Duration = Duration::from_secs(180);

/// Most commits `pick_continue` skips in a row for having nothing to commit.
const MAX_SKIPS: usize = 256;

// ───────────────────────────── validation ─────────────────────────────

/// A full object id: 40 (SHA-1) or 64 (SHA-256) hex digits. An abbreviation is refused: git would
/// take a branch or tag of that name for it.
fn check_oid(oid: &str) -> AppResult<()> {
    if matches!(oid.len(), 40 | 64) && oid.bytes().all(|b| b.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err(AppError::msg(format!("invalid object id: {oid}")))
    }
}

/// Repo-relative paths as `git status -z` prints them: not empty, not an option, not absolute, no `..`
/// component. A trailing `/` (an untracked nested repository) is allowed.
pub fn valid_paths(paths: &[String]) -> bool {
    !paths.is_empty()
        && paths.iter().all(|p| {
            !p.is_empty()
                && !p.starts_with('-')
                && !p.starts_with('/')
                && !p.split('/').any(|c| c == "..")
        })
}

fn check_paths(paths: &[String]) -> AppResult<()> {
    if valid_paths(paths) {
        Ok(())
    } else {
        Err(AppError::msg("invalid path"))
    }
}

/// Git's rules for a branch or tag name (`git check-ref-format`): not empty, not starting with `-`,
/// no component starting with `.` or ending in `.lock`, no `..`, `@{` or `//`, not `@`, no ASCII
/// control characters, space or `~ ^ : ? * [ \`, no leading or trailing `/`, not ending with `.`.
pub fn valid_ref_name(name: &str) -> bool {
    if name.is_empty() || name == "@" || name.starts_with('-') || name.starts_with('/') || name.ends_with('/') {
        return false;
    }
    if name.ends_with('.') || name.contains("..") || name.contains("@{") || name.contains("//") {
        return false;
    }
    if name.bytes().any(|b| b < 0x20 || b == 0x7f || b" ~^:?*[\\".contains(&b)) {
        return false;
    }
    name.split('/').all(|c| !c.starts_with('.') && !c.ends_with(".lock"))
}

fn check_branch_name(name: &str) -> AppResult<()> {
    if valid_ref_name(name) {
        Ok(())
    } else {
        Err(AppError::msg(format!("invalid branch name: {name}")))
    }
}

/// A remote branch by its full name, `refs/remotes/<remote>/<branch>`.
fn check_remote_ref(full_name: &str) -> AppResult<()> {
    match full_name.strip_prefix("refs/remotes/") {
        Some(rest) if rest.contains('/') && valid_ref_name(rest) => Ok(()),
        _ => Err(AppError::msg(format!("invalid remote branch: {full_name}"))),
    }
}

/// Remote names as passed on the command line: not empty, not an option, no whitespace.
fn check_remote(remote: &str) -> AppResult<()> {
    if remote.is_empty() || remote.starts_with('-') || remote.chars().any(char::is_whitespace) {
        Err(AppError::msg(format!("invalid remote name: {remote}")))
    } else {
        Ok(())
    }
}

// ───────────────────────────── network ─────────────────────────────

/// A network operation: no process slot (it may run for minutes), a deadline, stopped when the repo's
/// last tab closes.
fn net(git: Git, stop: Arc<AtomicBool>) -> Git {
    git.unslotted().timeout(NET_TIMEOUT).cancel_on(stop)
}

fn result(out: GitOutput) -> OpResult {
    OpResult { stdout: lossy(&out.stdout).trim().to_string(), stderr: lossy(&out.stderr).trim().to_string() }
}

/// The command may need objects that a partial clone does not hold yet (it writes files from them, or
/// merges): they are fetched on demand, as plain git does. Every other git runs with
/// `GIT_NO_LAZY_FETCH=1` (`cmd`: reading never reaches for the network), under which such a command
/// fails — a checkout half-way, the files it removes already gone.
fn fetching(git: Git) -> Git {
    git.env("GIT_NO_LAZY_FETCH", "0")
}

// ───────────────────────────── index ─────────────────────────────

/// `git add -A -- <paths>`: stages changes, new files and removals of these paths (a conflicted path
/// is marked resolved).
pub fn stage(cwd: &Path, paths: &[String]) -> AppResult<()> {
    check_paths(paths)?;
    Git::new(cwd).args(["add", "-A", "--"]).args(paths).literal_paths().run().map(drop)
}

/// `git add -A`: everything, untracked files included.
pub fn stage_all(cwd: &Path) -> AppResult<()> {
    Git::new(cwd).args(["add", "-A"]).run().map(drop)
}

/// `git reset -q -- <paths>` (an unborn HEAD counts as the empty tree, so it unstages there too).
pub fn unstage(cwd: &Path, paths: &[String]) -> AppResult<()> {
    check_paths(paths)?;
    Git::new(cwd).args(["reset", "-q", "--"]).args(paths).literal_paths().run().map(drop)
}

/// Everything staged back to HEAD's version: `git reset -q -- .`. The pathspec matters: a bare
/// `git reset` also ends a merge / cherry-pick / revert in progress (it drops `MERGE_HEAD` and its
/// kin, so the next commit would be no merge and a stopped pick could not be continued).
///
/// Conflicts stay conflicts: while the index holds unmerged entries, only the paths staged without
/// one are reset (`git diff-index --cached --diff-filter=u`, fed to `git reset --pathspec-from-file`),
/// as a reset of an unmerged path would turn it into a plain modified file full of markers.
pub fn unstage_all(cwd: &Path) -> AppResult<()> {
    if !has_unmerged(cwd) {
        return Git::new(cwd).args(["reset", "-q", "--", "."]).run().map(drop);
    }
    let staged = Git::new(cwd)
        .args(["diff-index", "--cached", "--name-only", "-z", "--diff-filter=u", "HEAD", "--"])
        .run()?;
    if staged.is_empty() {
        return Ok(());
    }
    Git::new(cwd)
        .args(["reset", "-q", "--pathspec-from-file=-", "--pathspec-file-nul"])
        .stdin(staged)
        .literal_paths()
        .run()
        .map(drop)
}

/// `git commit -q -F -` with the message on stdin (`--amend` to replace the HEAD commit). No editor
/// ever opens.
pub fn commit(cwd: &Path, message: &str, amend: bool) -> AppResult<OpResult> {
    if message.trim().is_empty() {
        return Err(AppError::msg("empty commit message"));
    }
    let git = Git::new(cwd)
        .args(["commit", "-q", "-F", "-"])
        .args(amend.then_some("--amend"))
        .stdin(message.as_bytes().to_vec())
        .env("GIT_EDITOR", "true");
    git.output_checked().map(result)
}

// ───────────────────────────── branches ─────────────────────────────

/// Create `name` at the commit `start`: `git switch -q -c <name> <start>` when `checkout` (the caller
/// asks for it only when `start` is the HEAD commit, so the working tree does not change), else
/// `git branch -- <name> <start>`. The name is checked here and by `git check-ref-format --branch`.
pub fn create_branch(cwd: &Path, name: &str, start: &str, checkout: bool) -> AppResult<()> {
    let name = name.trim();
    check_branch_name(name)?;
    check_oid(start)?;
    let chk = Git::new(cwd).args(["check-ref-format", "--branch", name]).output()?;
    if chk.code != 0 {
        return Err(AppError::msg(format!("invalid branch name: {name}")));
    }
    if checkout {
        Git::new(cwd).args(["switch", "-q", "-c", name, start]).run()?;
    } else {
        Git::new(cwd).args(["branch", "--", name, start]).run()?;
    }
    Ok(())
}

/// `git tag -- <name> <oid>`: a lightweight tag on the commit `oid`. The name is checked here and by
/// `git check-ref-format`; git refuses a name that is taken. No editor ever opens: with `tag.gpgSign`
/// set, git wants a message and fails with its own words.
pub fn create_tag(cwd: &Path, name: &str, oid: &str) -> AppResult<()> {
    let name = name.trim();
    check_oid(oid)?;
    let invalid = || AppError::msg(format!("invalid tag name: {name}"));
    if !valid_ref_name(name) {
        return Err(invalid());
    }
    let chk = Git::new(cwd).args(["check-ref-format", &format!("refs/tags/{name}")]).output()?;
    if chk.code != 0 {
        return Err(invalid());
    }
    Git::new(cwd).args(["tag", "--", name, oid]).env("GIT_EDITOR", "true").run().map(drop)
}

// ───────────────────────────── HEAD ─────────────────────────────

/// Switch the worktree at `cwd`: `git switch -q --no-guess <branch>`, `git switch -q -c <name> --track
/// <upstream>` (a remote branch that has no local one yet) or `git switch -q --detach <oid>`. Local
/// changes come along; where the switch would overwrite them git refuses and names the files.
pub fn checkout(cwd: &Path, target: &CheckoutTarget) -> AppResult<()> {
    let git = Git::new(cwd).args(["switch", "-q"]);
    let git = match target {
        CheckoutTarget::Branch { name } => {
            check_branch_name(name)?;
            // No guessing: a branch deleted meanwhile is not created anew from a remote of that name.
            git.args(["--no-guess", name])
        }
        CheckoutTarget::Track { name, upstream } => {
            check_branch_name(name)?;
            check_remote_ref(upstream)?;
            git.args(["-c", name, "--track", upstream])
        }
        CheckoutTarget::Commit { oid } => {
            check_oid(oid)?;
            git.args(["--detach", oid])
        }
    };
    fetching(git).run().map(drop)
}

/// Commits reachable from HEAD but from no ref, nor from `except` (the commit about to be checked
/// out): what a switch away from a detached HEAD leaves on no branch, where only the reflog still
/// finds them. git itself just warns on its way out. 0 on a branch, and for an unborn HEAD.
pub fn unreferenced(cwd: &Path, except: Option<&str>) -> AppResult<u32> {
    if let Some(oid) = except {
        check_oid(oid)?;
    }
    let out = Git::new(cwd).args(["rev-list", "--count", "HEAD", "--not", "--glob=*"]).args(except).output()?;
    Ok(if out.code == 0 { lossy(&out.stdout).trim().parse().unwrap_or(0) } else { 0 })
}

/// `git reset -q --soft|--mixed|--hard <oid> --`: move the checked-out branch (HEAD itself when
/// detached) to the commit `oid`. Soft keeps the index and the files, so the difference shows as
/// staged; mixed keeps the files; hard keeps neither: uncommitted changes of tracked files are lost.
pub fn reset(cwd: &Path, oid: &str, mode: ResetMode) -> AppResult<()> {
    check_oid(oid)?;
    let flag = match mode {
        ResetMode::Soft => "--soft",
        ResetMode::Mixed => "--mixed",
        ResetMode::Hard => "--hard",
    };
    // `--`: the oid is a revision, also next to a file of that name.
    fetching(Git::new(cwd).args(["reset", "-q", flag, oid, "--"])).run().map(drop)
}

// ───────────────────────────── cherry-pick / revert ─────────────────────────────

impl PickOp {
    fn command(self) -> &'static str {
        match self {
            PickOp::CherryPick => "cherry-pick",
            PickOp::Revert => "revert",
        }
    }

    /// The pseudo-ref naming the commit the operation is stopped at.
    fn stopped_at(self) -> &'static str {
        match self {
            PickOp::CherryPick => "CHERRY_PICK_HEAD",
            PickOp::Revert => "REVERT_HEAD",
        }
    }
}

/// How a `git cherry-pick` / `git revert` run ended.
#[derive(Debug, PartialEq, Eq)]
enum PickEnd {
    Done,
    /// Stopped on conflicts: in progress until it is continued or aborted.
    Conflicts,
    /// Nothing to commit: the changes are already there (cherry-pick) or already gone (revert).
    Empty,
    Failed,
}

/// Classify a run by its exit code and output (English: `cmd` sets `LC_ALL`), and by whether the index
/// holds unmerged entries afterwards (asked only when the output does not tell). Both commands exit 1
/// when they stop:
/// - on a commit that did not apply cleanly: the sequencer's `error: could not apply <oid>… <subject>`
///   (`could not revert`) on stderr, also when rerere or a directory rename left nothing unmerged;
/// - because `git commit` found nothing to commit: its status summary ends stdout ("nothing to
///   commit…", "no changes added to commit…", "nothing added to commit…").
///
/// Lines are matched at their start: a subject or a path may hold any of these words. Anything else
/// (exit 128: local changes in the way, a bad commit; a hook's refusal) is git's own error.
fn pick_end(code: i32, unmerged: impl FnOnce() -> bool, stdout: &str, stderr: &str) -> PickEnd {
    const STOPPED: [&str; 2] = ["error: could not apply ", "error: could not revert "];
    const NOTHING: [&str; 3] = ["nothing to commit", "no changes added to commit", "nothing added to commit"];
    if code == 0 {
        return PickEnd::Done;
    }
    if code != 1 {
        return PickEnd::Failed;
    }
    if stderr.lines().any(|l| STOPPED.iter().any(|m| l.starts_with(m))) || unmerged() {
        return PickEnd::Conflicts;
    }
    let summary = stdout.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or_default();
    if NOTHING.iter().any(|m| summary.starts_with(m)) {
        PickEnd::Empty
    } else {
        PickEnd::Failed
    }
}

/// Whether the index of the worktree at `cwd` holds unmerged entries (conflicts to resolve).
fn has_unmerged(cwd: &Path) -> bool {
    Git::new(cwd).args(["ls-files", "--unmerged"]).run().is_ok_and(|out| !out.is_empty())
}

/// Whether a file that the commit `op` is stopped at changes has unstaged changes in the worktree at
/// `cwd`: its resolution may only wait to be staged. Changes of other files are the user's own and
/// say nothing about it. False when there is no such commit.
fn resolution_unstaged(cwd: &Path, op: PickOp) -> bool {
    let names = |git: Git| -> Option<HashSet<Vec<u8>>> {
        let out = git.output().ok()?;
        (out.code == 0).then(|| split_records(&out.stdout, 0).into_iter().map(<[u8]>::to_vec).collect())
    };
    let Some(unstaged) = names(Git::new(cwd).args(["diff-files", "--name-only", "-z"])) else { return false };
    if unstaged.is_empty() {
        return false;
    }
    // Against its first parent (the mainline a merge commit is picked along); a root commit has none.
    let at = op.stopped_at();
    let changed = names(Git::new(cwd).args(["diff-tree", "-r", "--name-only", "-z", &format!("{at}^"), at]))
        .or_else(|| names(Git::new(cwd).args(["diff-tree", "-r", "--root", "--no-commit-id", "--name-only", "-z", at])));
    changed.is_some_and(|paths| paths.iter().any(|p| unstaged.contains(p)))
}

/// Whether `oid` is a boundary commit of a shallow clone (`shallow`: the text of `<common dir>/shallow`).
fn at_shallow_boundary(shallow: &str, oid: &str) -> bool {
    shallow.lines().any(|l| l.trim().eq_ignore_ascii_case(oid))
}

/// Run a cherry-pick / revert invocation (no editor ever opens; missing objects are fetched) and
/// classify how it ended; the code and message are git's own, for a failure.
fn run_pick(cwd: &Path, git: Git) -> AppResult<(PickEnd, i32, String)> {
    let out = fetching(git).env("GIT_EDITOR", "true").output()?;
    let (stdout, stderr) = (lossy(&out.stdout), lossy(&out.stderr));
    let end = pick_end(out.code, || has_unmerged(cwd), &stdout, &stderr);
    // `git commit` refuses on stdout (its status summary), with nothing on stderr.
    Ok((end, out.code, if stderr.trim().is_empty() { stdout } else { stderr }))
}

/// `git cherry-pick <oid>` or `git revert --no-edit <oid>` onto the HEAD of the opened worktree; a
/// merge commit is applied or undone against its first parent (`-m 1`). Conflicts leave it in progress
/// (`PickOutcome::Conflicts`). With nothing to commit it is `Empty` and over: git leaves an empty
/// cherry-pick pending (and an empty revert its message), which `--quit` forgets without moving HEAD
/// or touching a file.
///
/// Refused while another operation is in progress (`repo::operation`: a merge, a rebase, a sequence —
/// also one standing between two commits): the pick would become part of it, and aborting the pick
/// would abort all of it. Refused for a boundary commit of a shallow clone: its parents are cut off,
/// so git would take it for a root commit and apply (or undo) its whole tree.
pub fn pick(info: &RepoInfo, op: PickOp, oid: &str) -> AppResult<PickOutcome> {
    check_oid(oid)?;
    let cwd = Path::new(&info.path);
    let cmd = op.command();
    if let Some(running) = repo::operation(Path::new(&info.git_dir)) {
        return Err(AppError::msg(format!("{} in progress", running.kind)));
    }
    let shallow = std::fs::read(Path::new(&info.common_dir).join("shallow")).unwrap_or_default();
    if at_shallow_boundary(&lossy(&shallow), oid) {
        return Err(AppError::msg("the commit's parents are not in this shallow clone"));
    }
    // A merge commit has a second parent.
    let merge = Git::new(cwd).args(["rev-parse", "-q", "--verify", &format!("{oid}^2")]).output()?.code == 0;
    let git = Git::new(cwd)
        .arg(cmd)
        .args((op == PickOp::Revert).then_some("--no-edit"))
        .args(merge.then_some(["-m", "1"]).into_iter().flatten())
        .arg(oid);
    match run_pick(cwd, git)? {
        (PickEnd::Done, ..) => Ok(PickOutcome::Done),
        (PickEnd::Conflicts, ..) => Ok(PickOutcome::Conflicts),
        (PickEnd::Empty, ..) => {
            let _ = Git::new(cwd).args([cmd, "--quit"]).output();
            Ok(PickOutcome::Empty)
        }
        (PickEnd::Failed, code, stderr) => Err(AppError::Git { args: cmd.into(), code: Some(code), stderr }),
    }
}

/// `git cherry-pick --continue` / `git revert --continue` in the worktree at `cwd`, once the conflicts
/// are resolved: commits with the prepared message and goes on with the rest of a sequence. Conflicts
/// still in the index, or in a later commit, are `PickOutcome::Conflicts`. A commit whose resolution
/// left nothing to commit is skipped (`--skip`, git's own advice), like every following one that is
/// empty: `Empty` when the operation ended without any commit. Not while a file of that commit has
/// unstaged changes, though: its resolution may only wait to be staged, so that fails with `nothing
/// staged` and stays in progress.
pub fn pick_continue(cwd: &Path, op: PickOp) -> AppResult<PickOutcome> {
    if has_unmerged(cwd) {
        return Ok(PickOutcome::Conflicts);
    }
    let cmd = op.command();
    let head = || Git::new(cwd).args(["rev-parse", "-q", "--verify", "HEAD"]).run_string().unwrap_or_default();
    let before = head();
    let mut step = "--continue";
    // Each skip takes a commit off the sequence, so this ends.
    for _ in 0..=MAX_SKIPS {
        match run_pick(cwd, Git::new(cwd).args([cmd, step]))? {
            (PickEnd::Done, ..) if head() == before => return Ok(PickOutcome::Empty),
            (PickEnd::Done, ..) => return Ok(PickOutcome::Done),
            (PickEnd::Conflicts, ..) => return Ok(PickOutcome::Conflicts),
            // Only the commit the user resolved: the later ones of a sequence stop empty by themselves.
            (PickEnd::Empty, ..) if step == "--continue" && resolution_unstaged(cwd, op) => {
                return Err(AppError::msg("nothing staged"));
            }
            (PickEnd::Empty, ..) => step = "--skip",
            (PickEnd::Failed, code, stderr) => {
                return Err(AppError::Git { args: cmd.into(), code: Some(code), stderr });
            }
        }
    }
    Err(AppError::msg("nothing to commit"))
}

/// `git cherry-pick --abort` / `git revert --abort`: back to the state before the operation began
/// (all of a sequence: the commits it made so far go with it).
pub fn pick_abort(cwd: &Path, op: PickOp) -> AppResult<()> {
    fetching(Git::new(cwd).args([op.command(), "--abort"])).run().map(drop)
}

// ───────────────────────────── stashes ─────────────────────────────

/// `git stash push -q [-u] [-m <message>]`.
pub fn stash_push(cwd: &Path, include_untracked: bool, message: Option<&str>) -> AppResult<OpResult> {
    let message = message.map(str::trim).filter(|m| !m.is_empty());
    Git::new(cwd)
        .args(["stash", "push", "-q"])
        .args(include_untracked.then_some("-u"))
        .args(message.iter().flat_map(|m| ["-m", m]))
        .output_checked()
        .map(result)
}

/// `stash@{N}` after checking that it still is the stash `oid` (the list may have moved since the UI
/// read it: popping or dropping another entry would be a surprise).
fn stash_ref(cwd: &Path, index: u32, oid: &str) -> AppResult<String> {
    check_oid(oid)?;
    let name = format!("stash@{{{index}}}");
    let found = Git::new(cwd).args(["rev-parse", "-q", "--verify", &name]).allow_codes(&[1]).run_string()?;
    if found.trim() != oid {
        return Err(AppError::msg("stash list changed"));
    }
    Ok(name)
}

fn stash_op(cwd: &Path, op: &str, index: u32, oid: &str) -> AppResult<OpResult> {
    let name = stash_ref(cwd, index, oid)?;
    Git::new(cwd).args(["stash", op, "-q", &name]).output_checked().map(result)
}

/// `git stash pop -q stash@{N}` (a conflict leaves the stash entry in place and fails).
pub fn stash_pop(cwd: &Path, index: u32, oid: &str) -> AppResult<OpResult> {
    stash_op(cwd, "pop", index, oid)
}

/// `git stash apply -q stash@{N}`.
pub fn stash_apply(cwd: &Path, index: u32, oid: &str) -> AppResult<OpResult> {
    stash_op(cwd, "apply", index, oid)
}

/// `git stash drop -q stash@{N}`.
pub fn stash_drop(cwd: &Path, index: u32, oid: &str) -> AppResult<OpResult> {
    stash_op(cwd, "drop", index, oid)
}

/// `git fetch --all`, or `git fetch <remote>`.
pub fn fetch(cwd: &Path, remote: Option<&str>, stop: Arc<AtomicBool>) -> AppResult<OpResult> {
    let git = Git::new(cwd).arg("fetch");
    let git = match remote {
        Some(r) => {
            check_remote(r)?;
            git.arg(r)
        }
        None => git.arg("--all"),
    };
    net(git, stop).output_checked().map(result)
}

/// `git pull --no-edit [--rebase]` into the checked-out branch from its upstream. Divergent branches
/// without a `pull.rebase` / `pull.ff` setting fail with git's own message (nothing is merged then).
pub fn pull(cwd: &Path, rebase: bool, stop: Arc<AtomicBool>) -> AppResult<OpResult> {
    let git = Git::new(cwd).args(["pull", "--no-edit"]).args(rebase.then_some("--rebase")).env("GIT_EDITOR", "true");
    // The merge / rebase after the fetch writes files: in a partial clone it needs their blobs.
    net(fetching(git), stop).output_checked().map(result)
}

/// The background fetch: `git fetch --progress --no-write-fetch-head <remote>` for each of `remotes`,
/// one after the other, so that a remote that fails (a fork that is gone, a host behind a VPN) does
/// not keep the others from being fetched, and that stopping git stops the fetch itself (`git fetch
/// --all` leaves the work to a child per remote, which a signal to the parent does not reach).
///
/// Unlike the fetch the user asks for:
/// - It never prompts ([`unattended`]).
/// - It leaves `FETCH_HEAD` alone: a `git pull` reads what to merge from it between its own fetch and
///   its merge, and so does a `git merge FETCH_HEAD` typed in a terminal.
/// - A remote that `git fetch --all` would skip (`remote.<name>.skipFetchAll`) is skipped here too,
///   and so is a name git would take for an option.
/// - It has no time limit but one on silence ([`BACKGROUND_IDLE`]): `--progress` makes git report at
///   least every second while data flows, so one that says nothing hangs.
/// - It ends early, without an error, once `cancel` (a fetch / pull / push of the user's goes first,
///   or a newer background fetch began) or `stop` (the repo's last tab closed) is set: the remotes it
///   did not get to are missing from the answer. git is asked to end then, not killed (`cmd::end`): it
///   removes the ref locks and the pack's `.keep` file it holds.
///
/// A remote that fails is no error either: its entry carries git's message.
pub fn fetch_background(
    cwd: &Path,
    remotes: &[String],
    cancel: &Arc<AtomicBool>,
    stop: &Arc<AtomicBool>,
) -> AppResult<Vec<RemoteFetch>> {
    let cancelled = || cancel.load(Ordering::Relaxed) || stop.load(Ordering::Relaxed);
    let skipped = skipped_remotes(cwd);
    let mut done = Vec::with_capacity(remotes.len());
    for remote in remotes {
        if cancelled() {
            break;
        }
        let entry = |skipped, updated, error| RemoteFetch { remote: remote.clone(), skipped, updated, error };
        if remote.is_empty() || remote.starts_with('-') || skipped.contains(remote) {
            done.push(entry(true, false, None));
            continue;
        }
        let git = Git::new(cwd)
            .args(["-c", "credential.interactive=false", "fetch", "--progress", "--no-write-fetch-head"])
            .arg(remote)
            .unslotted()
            .idle_timeout(BACKGROUND_IDLE)
            .cancel_on(cancel.clone())
            .cancel_on(stop.clone());
        match unattended(git).output() {
            Ok(out) => {
                let stderr = lossy(&out.stderr);
                // One that failed may have moved refs all the same (another ref was in its way).
                let error = (out.code != 0).then(|| fetch_failure(&stderr, out.code));
                done.push(entry(false, reports_updates(&stderr), error));
            }
            // Stopped, not failed: the remote is fetched by the next background fetch.
            Err(_) if cancelled() => break,
            Err(e) => done.push(entry(false, false, Some(e.to_string()))),
        }
    }
    Ok(done)
}

/// A fetch nobody asked for must not ask for anything either. Next to the `GIT_TERMINAL_PROMPT=0`
/// every git runs with: no askpass program (an empty `GIT_ASKPASS` also overrides `core.askPass` and
/// `SSH_ASKPASS` for git's own prompts, and ssh is told not to use one for a passphrase or a host
/// key), no credential helper that opens a window (`credential.interactive=false`, which git 2.47+
/// and helpers such as Git Credential Manager honor, plus the latter's own switch). The command line
/// starts with that `-c`, before `fetch`. Stored credentials, agents and keys work as in any fetch;
/// where one is missing this fetch fails instead of opening a window every few minutes.
fn unattended(git: Git) -> Git {
    git.env("GIT_ASKPASS", "").env("SSH_ASKPASS_REQUIRE", "never").env("GCM_INTERACTIVE", "never")
}

/// What is left of a stderr line that progress rewrote in place: the text after its last `\r`,
/// without the blanks git pads it with.
fn shown(line: &str) -> &str {
    line.rsplit('\r').next().unwrap_or(line).trim_end()
}

/// Whether a `git fetch` changed a ref, by its stderr: the updates (new, moved and pruned branches
/// and tags) are listed under a `From <url>` line, which a fetch that found nothing new does not
/// print (English: `cmd` sets `LC_ALL`).
fn reports_updates(stderr: &str) -> bool {
    stderr.lines().any(|l| shown(l).starts_with("From "))
}

/// Whether a stderr line of `git fetch --progress` is progress, not a message: `<Title>: <count>…`
/// ("Receiving objects:  45% (555/1234)", "remote: Enumerating objects: 5, done.") or the remote's
/// closing "Total 5 (delta 1), reused 0 …".
fn is_progress(line: &str) -> bool {
    let line = line.strip_prefix("remote: ").unwrap_or(line);
    let counts = |s: &str| s.trim_start().starts_with(|c: char| c.is_ascii_digit());
    match line.split_once(": ") {
        Some((title, rest)) => {
            title.starts_with(|c: char| c.is_ascii_uppercase())
                && title.chars().all(|c| c.is_ascii_alphabetic() || c == ' ')
                && counts(rest)
        }
        None => line.strip_prefix("Total ").is_some_and(counts),
    }
}

/// What a background fetch that failed says: git's stderr without the progress it was asked for,
/// else its exit code.
fn fetch_failure(stderr: &str, code: i32) -> String {
    let lines: Vec<&str> = stderr.lines().map(shown).filter(|l| !l.is_empty() && !is_progress(l)).collect();
    if lines.is_empty() {
        format!("git fetch failed (exit {code})")
    } else {
        lines.join("\n")
    }
}

/// The remotes `git fetch --all` leaves out: `remote.<name>.skipFetchAll` / `skipDefaultUpdate`.
/// None when the config cannot be read (a value that is no boolean fails the fetch itself, in git's
/// own words).
fn skipped_remotes(cwd: &Path) -> HashSet<String> {
    let out = Git::new(cwd)
        .args(["config", "-z", "--type=bool", "--get-regexp", r"^remote\..*\.skip(defaultupdate|fetchall)$"])
        .output();
    match out {
        Ok(out) if out.code == 0 => parse_skipped_remotes(&out.stdout),
        _ => HashSet::new(),
    }
}

/// Remote names from `config -z --type=bool --get-regexp ^remote\..*\.skip(defaultupdate|fetchall)$`:
/// records `remote.<name>.<key>\n<true|false>\0`, the key lower-cased by git, the name (which may
/// hold `.` and `/`) as written. Both keys set one switch, so the last record of a remote decides, as
/// in git.
pub fn parse_skipped_remotes(buf: &[u8]) -> HashSet<String> {
    let mut skipped = HashSet::new();
    for record in split_records(buf, 0) {
        let Some(nl) = record.iter().position(|b| *b == b'\n') else { continue };
        let (key, value) = (&record[..nl], &record[nl + 1..]);
        let Some(rest) = key.strip_prefix(b"remote.") else { continue };
        let Some(name) = rest.strip_suffix(b".skipfetchall").or_else(|| rest.strip_suffix(b".skipdefaultupdate"))
        else {
            continue;
        };
        if name.is_empty() {
            continue;
        }
        if value == b"true" {
            skipped.insert(lossy(name));
        } else {
            skipped.remove(&lossy(name));
        }
    }
    skipped
}

/// `git push [-u] [<remote> [<branch>]]`: the checked-out branch to its upstream, or with `remote`
/// and `branch` (and `set_upstream`) a first push that records the upstream.
pub fn push(
    cwd: &Path,
    remote: Option<&str>,
    branch: Option<&str>,
    set_upstream: bool,
    stop: Arc<AtomicBool>,
) -> AppResult<OpResult> {
    if let Some(r) = remote {
        check_remote(r)?;
    }
    if let Some(b) = branch {
        check_branch_name(b)?;
    }
    if branch.is_some() && remote.is_none() {
        return Err(AppError::msg("a branch needs a remote"));
    }
    let git = Git::new(cwd)
        .arg("push")
        .args(set_upstream.then_some("-u"))
        .args(remote)
        .args(branch);
    net(git, stop).output_checked().map(result)
}

// ───────────────────────────── deleting branches and tags ─────────────────────────────

fn check_tag_name(name: &str) -> AppResult<()> {
    if valid_ref_name(name) {
        Ok(())
    } else {
        Err(AppError::msg(format!("invalid tag name: {name}")))
    }
}

/// A branch, a remote branch or a tag by its full name: `refs/heads/…`, `refs/remotes/…`, `refs/tags/…`.
fn check_full_ref(full_name: &str) -> AppResult<()> {
    let name = ["refs/heads/", "refs/remotes/", "refs/tags/"].iter().find_map(|p| full_name.strip_prefix(p));
    match name {
        Some(name) if valid_ref_name(name) => Ok(()),
        _ => Err(AppError::msg(format!("invalid ref: {full_name}"))),
    }
}

/// Commits that only the ref `full_name` reaches, of those reachable from `oid` (the commit the UI
/// shows that ref at): `git rev-list --count <oid> --not` every other branch, remote branch and tag,
/// and the stash. Deleting the ref leaves them on no branch — out of the graph, for git to prune one
/// day — so the frontend asks first, as before a checkout away from a detached HEAD
/// ([`unreferenced`]). 0 when another of those refs holds them too.
///
/// Only the refs the graph shows hold anything here. A ref of another namespace keeps the objects but
/// puts them on no branch: with `git maintenance`'s prefetch, `refs/prefetch/remotes/<remote>/<branch>`
/// mirrors every remote branch, and counted in it would hide every commit a remote branch is alone to
/// hold. Nor does a detached HEAD hold any: it is no branch.
pub fn exclusive(cwd: &Path, full_name: &str, oid: &str) -> AppResult<u32> {
    check_full_ref(full_name)?;
    check_oid(oid)?;
    // An exclusion lasts until the next `--glob`. A ref name holds none of the characters a glob
    // reads (`valid_ref_name`), so this one leaves out that ref alone.
    let skip = format!("--exclude={full_name}");
    let out = Git::new(cwd)
        .args(["rev-list", "--count", oid, "--not"])
        .args([skip.as_str(), "--glob=refs/heads/*"])
        // `<remote>/HEAD` is a second name of one of that remote's branches: of this one, perhaps.
        .args([skip.as_str(), "--exclude=refs/remotes/*/HEAD", "--glob=refs/remotes/*"])
        .args([skip.as_str(), "--glob=refs/tags/*"])
        // The stash, if there is one: a pattern without a wildcard would stand for `refs/stash/*`.
        .arg("--glob=refs/stas[h]")
        .run_string()?;
    Ok(out.trim().parse().unwrap_or(0))
}

/// The ref `full_name` (shown as `name`) still stands for the commit `oid` — a tag peeled to it. The
/// UI read it there, and asked about deleting it there: one that moved since is not deleted.
fn check_still_at(cwd: &Path, full_name: &str, name: &str, oid: &str) -> AppResult<()> {
    let found = Git::new(cwd)
        .args(["rev-parse", "-q", "--verify", &format!("{full_name}^{{}}")])
        .allow_codes(&[1])
        .run_string()?;
    match found.trim() {
        "" => Err(AppError::msg(format!("{name} is gone"))),
        at if at.eq_ignore_ascii_case(oid) => Ok(()),
        _ => Err(AppError::msg(format!("{name} has moved"))),
    }
}

/// Delete the local branch `name`: `git branch -D -- <name>` — merged or not: the frontend has asked,
/// naming the commits only this branch reaches ([`exclusive`]). Refused when the branch is no longer
/// at `oid`, the commit that question was about. git itself refuses a branch that is checked out in a
/// worktree, or being rebased or bisected in one. The branch's upstream settings (`branch.<name>.*`)
/// and its reflog go with it.
pub fn delete_branch(cwd: &Path, name: &str, oid: &str) -> AppResult<()> {
    check_branch_name(name)?;
    check_oid(oid)?;
    check_still_at(cwd, &format!("refs/heads/{name}"), name, oid)?;
    Git::new(cwd).args(["branch", "-D", "--", name]).run().map(drop)
}

/// Delete the tag `name`: `git tag -d -- <name>`. Refused when it no longer stands for the commit
/// `oid`.
pub fn delete_tag(cwd: &Path, name: &str, oid: &str) -> AppResult<()> {
    check_tag_name(name)?;
    check_oid(oid)?;
    check_still_at(cwd, &format!("refs/tags/{name}"), name, oid)?;
    Git::new(cwd).args(["tag", "-d", "--", name]).run().map(drop)
}

/// Whether a `git push --force-with-lease` was refused for its lease: the remote's ref is not where
/// the lease expects it. git says so in that ref's status line on stderr, ` ! [rejected] (delete) ->
/// <ref> (stale info)` (English: `cmd` sets `LC_ALL`; uncolored: the caller turns `color.transport`
/// off). A refusal by the remote itself — a protected branch, a hook — reads `! [remote rejected]`
/// and is git's own error.
fn lease_refused(stderr: &str) -> bool {
    stderr.lines().map(|l| shown(l).trim_start()).any(|l| l.starts_with("! [rejected]") && l.ends_with("(stale info)"))
}

/// A remote of this repository, by name (`git remote get-url`). git takes a name that is none for a
/// path or a URL: a push "to" a remote-tracking branch's remote that was never configured, or is no
/// longer (leftover refs, git-svn), would go to a directory of that name in the worktree.
fn check_configured_remote(cwd: &Path, remote: &str) -> AppResult<()> {
    check_remote(remote)?;
    if Git::new(cwd).args(["remote", "get-url", remote]).output()?.code == 0 {
        Ok(())
    } else {
        Err(AppError::msg(format!("no such remote: {remote}")))
    }
}

/// Delete the branch `branch` on `remote`, which the UI shows at `oid` (its remote-tracking branch):
/// `git push --force-with-lease=refs/heads/<branch>:<oid> <remote> :refs/heads/<branch>`. With the
/// lease the remote deletes its branch only while it is at `oid`: commits pushed there since the last
/// fetch — which nobody here has seen, and which the frontend's question did not count — do not go
/// with it. git removes the remote-tracking branch once the remote has deleted its own; one that the
/// remote's fetch refspec does not cover (a single-branch clone) is removed here.
///
/// A lease the remote refuses ([`lease_refused`]) has two causes: the branch moved there, or it is
/// gone there already — deleted by someone else, or by the host once its pull request was merged —
/// and only the remote-tracking branch is left here, which a fetch without `--prune` keeps for good.
/// A second push tells them apart, with a lease that expects no branch at all: a dry run, which sends
/// nothing and runs no hook, to the very URL that refused (a `pushurl` may differ from the one
/// `git ls-remote` would ask). Refused again, the branch moved: an error, a fetch shows where to.
/// Accepted, it is gone: the remote-tracking branch is then deleted (`git update-ref -d`, while it
/// still is at `oid`), which leaves what was asked for.
pub fn delete_remote_branch(
    cwd: &Path,
    remote: &str,
    branch: &str,
    oid: &str,
    stop: Arc<AtomicBool>,
) -> AppResult<()> {
    check_configured_remote(cwd, remote)?;
    check_branch_name(branch)?;
    check_oid(oid)?;
    let dst = format!("refs/heads/{branch}");
    let tracking = format!("refs/remotes/{remote}/{branch}");
    // The deletion, on the condition that the remote's branch is at `expect` (empty: that there is
    // none). `probe`: only asked, not done.
    let push = |expect: &str, probe: bool| {
        let git = Git::new(cwd)
            // Its status lines are read below: without colors, whatever `color.transport` says.
            .args(["-c", "color.transport=never", "push"])
            .args(probe.then_some(["--dry-run", "--no-verify"]).into_iter().flatten())
            .arg(format!("--force-with-lease={dst}:{expect}"))
            .arg(remote)
            .arg(format!(":{dst}"));
        net(git, stop.clone()).output()
    };
    let out = push(oid, false)?;
    if out.code == 0 {
        // Gone already where git removed it itself: this then fails, and nothing is amiss.
        let _ = Git::new(cwd).args(["update-ref", "-d", &tracking, oid]).output();
        return Ok(());
    }
    let stderr = lossy(&out.stderr);
    if !lease_refused(&stderr) {
        return Err(AppError::Git { args: "push".into(), code: Some(out.code), stderr });
    }
    let probe = push("", true)?;
    if probe.code != 0 {
        let stderr = lossy(&probe.stderr);
        return Err(if lease_refused(&stderr) {
            AppError::msg(format!("{branch} has moved on {remote}: fetch first"))
        } else {
            AppError::Git { args: "push".into(), code: Some(probe.code), stderr }
        });
    }
    Git::new(cwd).args(["update-ref", "-d", &tracking, oid]).run().map(drop)
}

/// Delete the tag `name` on `remote`: `git push <remote> :refs/tags/<name>`, whatever it points at
/// there (a tag is deleted from a remote to move it: no lease ties it to the tag here). A remote that
/// has no such tag only warns (`deleting a non-existent ref`) and git succeeds: the frontend reads
/// that from the output.
pub fn delete_remote_tag(cwd: &Path, remote: &str, name: &str, stop: Arc<AtomicBool>) -> AppResult<OpResult> {
    check_configured_remote(cwd, remote)?;
    check_tag_name(name)?;
    let git = Git::new(cwd).arg("push").arg(remote).arg(format!(":refs/tags/{name}"));
    net(git, stop).output_checked().map(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ref_names() {
        for ok in ["main", "feature/x", "a.b", "v1.0", "a/b/c", "fix-1_2", "한글", "api/3.2611"] {
            assert!(valid_ref_name(ok), "{ok}");
        }
        for bad in [
            "", "-x", ".hidden", "a..b", "a/", "/a", "x.lock", "a b", "a~b", "a^b", "a:b", "a?b", "a*b", "a[b", "a\\b",
            "@", "a@{b", "a//b", "a.", "dir/.x", "a\tb", "a\u{7f}b", "dir/x.lock/y",
        ] {
            assert!(!valid_ref_name(bad), "{bad:?}");
        }
    }

    #[test]
    fn remote_refs() {
        assert!(check_remote_ref("refs/remotes/origin/main").is_ok());
        assert!(check_remote_ref("refs/remotes/team/upstream/feature/x").is_ok());
        for bad in [
            "origin/main",
            "refs/heads/main",
            "refs/remotes/origin",
            "refs/remotes/origin/",
            "refs/remotes/-o/x",
            "refs/remotes/origin/a..b",
            "refs/remotes/origin/a b",
            "",
        ] {
            assert!(check_remote_ref(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn pick_endings() {
        // Outputs of git 2.48 (LC_ALL=C.UTF-8).
        const CONFLICT_OUT: &str = "Auto-merging a.txt\nCONFLICT (content): Merge conflict in a.txt\n";
        const CONFLICT_ERR: &str = "error: could not apply 0a3f4bc... feature change a\n\
            hint: After resolving the conflicts, mark them with\n\
            hint: \"git add/rm <pathspec>\", then run\n\
            hint: \"git cherry-pick --continue\".\n";
        const REVERT_ERR: &str = "error: could not revert 1b221e4... main change a\nhint: After resolving the conflicts\n";
        const EMPTY_ERR: &str = "The previous cherry-pick is now empty, possibly due to conflict resolution.\n\
            If you wish to commit it anyway, use:\n\n    git commit --allow-empty\n\n\
            Otherwise, please use 'git cherry-pick --skip'\n";
        const CLEAN: &str = "On branch main\nnothing to commit, working tree clean\n";
        const UNSTAGED: &str = "On branch main\nChanges not staged for commit:\n\tmodified:   z.txt\n\n\
            no changes added to commit (use \"git add\" and/or \"git commit -a\")\n";
        const UNTRACKED: &str = "On branch main\nUntracked files:\n\tu.txt\n\n\
            nothing added to commit but untracked files present (use \"git add\" to track)\n";
        let (yes, no) = (|| true, || false);
        // The index is asked only when the output of a stopped run (exit 1) does not tell.
        let unasked = || -> bool { panic!("asked for unmerged entries") };

        assert_eq!(pick_end(0, unasked, "[main 5d695ee] feature adds b\n", ""), PickEnd::Done);
        // Stopped on a commit that did not apply: the sequencer says so, with or without unmerged
        // entries left (rerere.autoupdate, an implicit directory rename).
        assert_eq!(pick_end(1, unasked, CONFLICT_OUT, CONFLICT_ERR), PickEnd::Conflicts);
        assert_eq!(pick_end(1, unasked, CONFLICT_OUT, REVERT_ERR), PickEnd::Conflicts);
        // Unmerged entries decide where the output says nothing (merge.verbosity 0 keeps stdout quiet).
        assert_eq!(pick_end(1, yes, "", ""), PickEnd::Conflicts);
        assert_eq!(pick_end(1, yes, CLEAN, ""), PickEnd::Conflicts);
        // An empty cherry-pick (stderr advice), whatever else the tree holds; an empty revert (status only).
        assert_eq!(pick_end(1, no, CLEAN, EMPTY_ERR), PickEnd::Empty);
        assert_eq!(pick_end(1, no, UNSTAGED, EMPTY_ERR), PickEnd::Empty);
        assert_eq!(pick_end(1, no, CLEAN, ""), PickEnd::Empty);
        assert_eq!(pick_end(1, no, UNSTAGED, ""), PickEnd::Empty);
        assert_eq!(pick_end(1, no, UNTRACKED, ""), PickEnd::Empty);
        // The words in a subject or a path are no verdict: a conflict whose subject says "is now
        // empty", a hook refusing a commit that touches a file called "nothing to commit".
        let subject = "error: could not apply 0a3f4bc... the cache is now empty, nothing to commit\n";
        assert_eq!(pick_end(1, unasked, CONFLICT_OUT, subject), PickEnd::Conflicts);
        let path = "On branch main\nChanges to be committed:\n\tmodified:   nothing to commit.txt\n\n";
        assert_eq!(pick_end(1, no, path, "commit-msg: no ticket id\n"), PickEnd::Failed);
        assert_eq!(pick_end(1, no, " nothing to commit\n", ""), PickEnd::Failed);
        // A hook refused the commit.
        assert_eq!(pick_end(1, no, "", "commit-msg: no ticket id\n"), PickEnd::Failed);
        // Fatal errors exit 128.
        let in_the_way = "error: Your local changes to the following files would be overwritten by merge:\n\ta.txt\n\
            Please commit your changes or stash them before you merge.\nAborting\nfatal: cherry-pick failed\n";
        assert_eq!(pick_end(128, unasked, "", in_the_way), PickEnd::Failed);
        let no_mainline = "error: commit 59bc8bf is a merge but no -m option was given.\nfatal: revert failed\n";
        assert_eq!(pick_end(128, unasked, "", no_mainline), PickEnd::Failed);
        assert_eq!(pick_end(-1, unasked, "", ""), PickEnd::Failed);
    }

    /// stderr of `git fetch --progress` on a pipe (git 2.48, LC_ALL=C.UTF-8): each progress line is
    /// rewritten after a `\r` and padded with blanks.
    const FETCH_PROGRESS: &str = "remote: Enumerating objects: 13, done.        \n\
        remote: Counting objects:   7% (1/13)        \rremote: Counting objects: 100% (13/13)        \r\
        remote: Counting objects: 100% (13/13), done.        \n\
        remote: Compressing objects:   8% (1/12)        \rremote: Compressing objects: 100% (12/12), done.        \n\
        remote: Total 12 (delta 3), reused 0 (delta 0), pack-reused 0 (from 0)        \n";

    #[test]
    fn fetch_updates() {
        let refs = "From github.com:team/repo\n   5e37cce..60fae9e  main       -> origin/main\n \
            * [new branch]      feat       -> origin/feat\n * [new tag]         v1         -> v1\n";
        assert!(reports_updates(refs));
        assert!(reports_updates(&format!("{FETCH_PROGRESS}{refs}")));
        assert!(reports_updates("From /p/up\n - [deleted]         (none)     -> origin/gone\n"));
        // Nothing new prints nothing, or progress alone; a warning is no update.
        assert!(!reports_updates(""));
        assert!(!reports_updates(FETCH_PROGRESS));
        assert!(!reports_updates("warning: redirecting to https://example.com/repo.git/\n"));
        assert!(!reports_updates("   From here on, indented\n"));
    }

    #[test]
    fn fetch_failures() {
        // The message without the progress around it; an error that cuts a progress line off follows
        // its last `\r`.
        let cut = format!(
            "{FETCH_PROGRESS}Receiving objects:  45% (555/1234), 10.00 MiB | 1.00 MiB/s        \r\
             fatal: the remote end hung up unexpectedly\nfatal: early EOF\nfatal: fetch-pack: invalid index-pack output\n"
        );
        assert_eq!(
            fetch_failure(&cut, 128),
            "fatal: the remote end hung up unexpectedly\nfatal: early EOF\nfatal: fetch-pack: invalid index-pack output"
        );
        // One ref was in the way of another: the rest of the fetch is told as it is.
        let partial = format!(
            "{FETCH_PROGRESS}Unpacking objects: 100% (3/3), 250 bytes | 125.00 KiB/s, done.\n\
             From /p/up\n   5e37cce..60fae9e  main       -> origin/main\n\
             error: cannot lock ref 'refs/remotes/origin/topic/sub': 'refs/remotes/origin/topic' exists\n \
             ! [new branch]      topic/sub  -> origin/topic/sub  (unable to update local ref)\n"
        );
        assert!(reports_updates(&partial));
        assert_eq!(
            fetch_failure(&partial, 1),
            "From /p/up\n   5e37cce..60fae9e  main       -> origin/main\n\
             error: cannot lock ref 'refs/remotes/origin/topic/sub': 'refs/remotes/origin/topic' exists\n \
             ! [new branch]      topic/sub  -> origin/topic/sub  (unable to update local ref)"
        );
        // Messages that hold a colon are no progress.
        for line in [
            "fatal: unable to access 'https://example.com/r.git/': Could not resolve host: example.com",
            "ssh: connect to host example.com port 22: Operation timed out",
            "remote: Repository not found.",
            "ERROR: Repository not found.",
            "Warning: Permanently added 'example.com' (ED25519) to the list of known hosts.",
            "Received disconnect from 192.0.2.1 port 22:2: Too many authentication failures",
            "git@example.com: Permission denied (publickey).",
        ] {
            assert!(!is_progress(line), "{line}");
            assert_eq!(fetch_failure(&format!("{line}\n"), 128), line);
        }
        for line in [
            "Receiving objects:  45% (555/1234), 10.00 MiB | 1.00 MiB/s",
            "Resolving deltas: 100% (2/2), done.",
            "Checking connectivity: 5, done.",
            "remote: Enumerating objects: 13, done.",
            "remote: Total 12 (delta 3), reused 0 (delta 0), pack-reused 0 (from 0)",
        ] {
            assert!(is_progress(line), "{line}");
        }
        // Nothing but progress, or nothing at all: the exit code.
        assert_eq!(fetch_failure(FETCH_PROGRESS, 1), "git fetch failed (exit 1)");
        assert_eq!(fetch_failure("", 128), "git fetch failed (exit 128)");
    }

    #[test]
    fn skipped_remote_names() {
        // Real output: a valueless key reads `true`, names keep their dots, slashes and case.
        let out = b"remote.team/up.stream.skipfetchall\ntrue\0remote.dead.skipdefaultupdate\ntrue\0\
            remote.dead.skipfetchall\nfalse\0remote.origin.skipfetchall\nfalse\0remote.Fork.skipdefaultupdate\ntrue\0";
        let skipped = parse_skipped_remotes(out);
        let mut names: Vec<&str> = skipped.iter().map(String::as_str).collect();
        names.sort_unstable();
        // `dead`: its last record decides, as `git fetch --all` takes it.
        assert_eq!(names, ["Fork", "team/up.stream"]);
        assert!(parse_skipped_remotes(b"").is_empty());
        // Records of another shape are left out.
        assert!(parse_skipped_remotes(b"remote..skipfetchall\ntrue\0remote.x.url\ntrue\0remote.y.skipfetchall\0").is_empty());
    }

    #[test]
    fn full_refs() {
        for ok in [
            "refs/heads/main",
            "refs/heads/feature/x",
            "refs/remotes/origin/main",
            "refs/tags/v1.0",
            "refs/tags/api/3.2611",
        ] {
            assert!(check_full_ref(ok).is_ok(), "{ok}");
        }
        // No other namespace, no short name, nothing a glob or a revision would read.
        for bad in [
            "",
            "main",
            "HEAD",
            "refs/heads/",
            "refs/stash",
            "refs/notes/commits",
            "refs/heads/a*",
            "refs/heads/a..b",
            "refs/tags/v1^{}",
            "refs/heads/-x",
            "--glob=refs/heads/x",
        ] {
            assert!(check_full_ref(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn refused_leases() {
        // stderr of git 2.48 (LC_ALL=C.UTF-8). The remote's branch moved, or is gone: both read alike,
        // and so does the second push (which expects no branch) while the remote has one.
        let stale = "To example.com:team/repo.git\n ! [rejected]        (delete) -> topic (stale info)\n\
            error: failed to push some refs to 'example.com:team/repo.git'\n";
        assert!(lease_refused(stale));
        // The remote's own refusal, a failure before any ref was looked at and a deletion that went
        // through are none.
        let denied = "remote: error: denying ref deletion for refs/heads/topic        \n\
            To example.com:team/repo.git\n ! [remote rejected] topic (deletion prohibited)\n\
            error: failed to push some refs to 'example.com:team/repo.git'\n";
        assert!(!lease_refused(denied));
        let no_remote = "fatal: 'nosuch' does not appear to be a git repository\n\
            fatal: Could not read from remote repository.\n";
        assert!(!lease_refused(no_remote));
        assert!(!lease_refused("To example.com:team/repo.git\n - [deleted]         topic\n"));
        // A branch named after the reason is no reason.
        assert!(!lease_refused(" - [deleted]         (stale info)\n"));
        assert!(!lease_refused(""));
    }

    #[test]
    fn shallow_boundary() {
        let shallow = "4b825dc642cb6eb9a060e54bf8d69288fbee4904\n0158b9d26427d87bbfcf439861b77641c18c00f0\n";
        assert!(at_shallow_boundary(shallow, "0158b9d26427d87bbfcf439861b77641c18c00f0"));
        assert!(at_shallow_boundary(shallow, "4B825DC642CB6EB9A060E54BF8D69288FBEE4904"));
        assert!(!at_shallow_boundary(shallow, "c2c237a000c963ad9fb59950a4322c0744ef47a5"));
        assert!(!at_shallow_boundary("", "0158b9d26427d87bbfcf439861b77641c18c00f0"));
    }

    #[test]
    fn paths() {
        let s = |v: &[&str]| v.iter().map(|p| p.to_string()).collect::<Vec<_>>();
        assert!(valid_paths(&s(&["a.txt", "dir with space/f.txt", "nested/", "tab\tname.txt", "새 파일.txt"])));
        assert!(!valid_paths(&s(&[])));
        for bad in ["", "-flag", "/abs/path", "../up", "a/../b", "a/.."] {
            assert!(!valid_paths(&s(&[bad])), "{bad:?}");
            assert!(!valid_paths(&s(&["ok.txt", bad])), "{bad:?}");
        }
    }

    #[test]
    fn oids_and_remotes() {
        assert!(check_oid("4b825dc642cb6eb9a060e54bf8d69288fbee4904").is_ok());
        assert!(check_oid("6ef19b41225c5369f1c104d45d8d85efa9b057b53b14b4b9b939dd74decc5321").is_ok());
        // An abbreviation could be a branch name.
        assert!(check_oid("abcd").is_err());
        assert!(check_oid("4b825dc642cb6eb9a060e54bf8d69288fbee490").is_err());
        assert!(check_oid("4b825dc642cb6eb9a060e54bf8d69288fbee4904g").is_err());
        assert!(check_oid("HEAD").is_err());
        assert!(check_oid("stash@{0}").is_err());
        assert!(check_remote("origin").is_ok());
        assert!(check_remote("team/upstream").is_ok());
        assert!(check_remote("").is_err());
        assert!(check_remote("--all").is_err());
        assert!(check_remote("a b").is_err());
    }
}
