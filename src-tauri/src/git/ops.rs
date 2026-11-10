//! The write operations the viewer offers: stage / unstage, commit, branch and tag creation, checkout,
//! reset, cherry-pick / revert (with continue and abort), stash and the network trio fetch / pull /
//! push. Every other command stays read-only. Each function runs one git process (plus a check or two
//! for stashes, names and how a cherry-pick ended) through `cmd::Git`, so the usual env and config
//! overrides apply (hooks, LFS filters and credential helpers resolve on the login-shell PATH, see
//! `cmd::path_env`); the index lock is a required lock, which `--no-optional-locks` leaves alone.

use crate::error::{AppError, AppResult};
use crate::git::cmd::{lossy, Git, GitOutput};
use crate::model::{CheckoutTarget, OpResult, PickOp, PickOutcome, ResetMode};
use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Duration;

/// Network operations are killed after this long.
const NET_TIMEOUT: Duration = Duration::from_secs(600);

// ───────────────────────────── validation ─────────────────────────────

/// `^[0-9a-fA-F]{4,64}$`.
fn check_oid(oid: &str) -> AppResult<()> {
    if (4..=64).contains(&oid.len()) && oid.bytes().all(|b| b.is_ascii_hexdigit()) {
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

/// A network operation: no process slot (it may run for minutes), a deadline, killed when the repo's
/// last tab closes.
fn net(git: Git, stop: Arc<AtomicBool>) -> Git {
    git.unslotted().timeout(NET_TIMEOUT).cancel_on(stop)
}

fn result(out: GitOutput) -> OpResult {
    OpResult { stdout: lossy(&out.stdout).trim().to_string(), stderr: lossy(&out.stderr).trim().to_string() }
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

/// `git reset -q`: the whole index back to HEAD.
pub fn unstage_all(cwd: &Path) -> AppResult<()> {
    Git::new(cwd).args(["reset", "-q"]).run().map(drop)
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
    git.run().map(drop)
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
    Git::new(cwd).args(["reset", "-q", flag, oid, "--"]).run().map(drop)
}

// ───────────────────────────── cherry-pick / revert ─────────────────────────────

impl PickOp {
    fn command(self) -> &'static str {
        match self {
            PickOp::CherryPick => "cherry-pick",
            PickOp::Revert => "revert",
        }
    }

    /// The pseudo-ref git keeps while the operation is stopped.
    fn pending_ref(self) -> &'static str {
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

/// Classify a run by its exit code, by whether the index holds unmerged entries afterwards (asked only
/// when that decides) and by its output (English: `cmd` sets `LC_ALL`). Both commands exit 1 when they
/// stop: on conflicts, or because `git commit` found nothing to commit. That refusal is "The previous
/// cherry-pick is now empty" on stderr for a cherry-pick and only the status summary on stdout for a
/// revert ("nothing to commit", "no changes added to commit", "nothing added to commit"). Anything
/// else (local changes in the way, a bad commit, a hook) is git's own error.
fn pick_end(code: i32, unmerged: impl FnOnce() -> bool, stdout: &str, stderr: &str) -> PickEnd {
    const NOTHING: [&str; 3] = ["nothing to commit", "no changes added to commit", "nothing added to commit"];
    if code == 0 {
        return PickEnd::Done;
    }
    if code != 1 {
        return PickEnd::Failed;
    }
    if unmerged() {
        return PickEnd::Conflicts;
    }
    if stderr.contains("is now empty") || NOTHING.iter().any(|m| stdout.contains(m)) {
        PickEnd::Empty
    } else {
        PickEnd::Failed
    }
}

/// Whether the index of the worktree at `cwd` holds unmerged entries (conflicts to resolve).
fn has_unmerged(cwd: &Path) -> bool {
    Git::new(cwd).args(["ls-files", "--unmerged"]).run().is_ok_and(|out| !out.is_empty())
}

/// Run a cherry-pick / revert invocation (no editor ever opens) and classify how it ended; the
/// message is git's own, for a failure.
fn run_pick(cwd: &Path, git: Git) -> AppResult<(PickEnd, i32, String)> {
    let out = git.env("GIT_EDITOR", "true").output()?;
    let (stdout, stderr) = (lossy(&out.stdout), lossy(&out.stderr));
    let end = pick_end(out.code, || has_unmerged(cwd), &stdout, &stderr);
    // `git commit` refuses on stdout (its status summary), with nothing on stderr.
    Ok((end, out.code, if stderr.trim().is_empty() { stdout } else { stderr }))
}

/// `git cherry-pick <oid>` or `git revert --no-edit <oid>` onto HEAD; a merge commit is applied or
/// undone against its first parent (`-m 1`). Conflicts leave the operation in progress
/// (`PickOutcome::Conflicts`). One with nothing to commit fails, and no longer is in progress: git
/// leaves an empty cherry-pick pending, with nothing to resolve.
pub fn pick(cwd: &Path, op: PickOp, oid: &str) -> AppResult<PickOutcome> {
    check_oid(oid)?;
    let cmd = op.command();
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
            // The index equals HEAD here, so the abort's `reset --merge` touches no file.
            let pending = Git::new(cwd).args(["rev-parse", "-q", "--verify", op.pending_ref()]).output();
            if pending.is_ok_and(|p| p.code == 0) {
                let _ = Git::new(cwd).args([cmd, "--abort"]).output();
            }
            Err(AppError::msg(format!("nothing to {cmd}")))
        }
        (PickEnd::Failed, code, stderr) => Err(AppError::Git { args: cmd.into(), code: Some(code), stderr }),
    }
}

/// `git cherry-pick --continue` / `git revert --continue` once the conflicts are resolved: commits
/// with the prepared message. Conflicts still in the index, or in a later commit of a sequence, are
/// `PickOutcome::Conflicts`. A resolution that leaves nothing to commit fails and stays in progress
/// (abort ends it).
pub fn pick_continue(cwd: &Path, op: PickOp) -> AppResult<PickOutcome> {
    if has_unmerged(cwd) {
        return Ok(PickOutcome::Conflicts);
    }
    let cmd = op.command();
    match run_pick(cwd, Git::new(cwd).args([cmd, "--continue"]))? {
        (PickEnd::Done, ..) => Ok(PickOutcome::Done),
        (PickEnd::Conflicts, ..) => Ok(PickOutcome::Conflicts),
        (PickEnd::Empty, ..) => Err(AppError::msg("nothing to commit")),
        (PickEnd::Failed, code, stderr) => Err(AppError::Git { args: cmd.into(), code: Some(code), stderr }),
    }
}

/// `git cherry-pick --abort` / `git revert --abort`: back to the state before the operation began.
pub fn pick_abort(cwd: &Path, op: PickOp) -> AppResult<()> {
    Git::new(cwd).args([op.command(), "--abort"]).run().map(drop)
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
    net(git, stop).output_checked().map(result)
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
        const EMPTY_ERR: &str = "The previous cherry-pick is now empty, possibly due to conflict resolution.\n\
            If you wish to commit it anyway, use:\n\n    git commit --allow-empty\n\n\
            Otherwise, please use 'git cherry-pick --skip'\n";
        const CLEAN: &str = "On branch main\nnothing to commit, working tree clean\n";
        const UNSTAGED: &str = "On branch main\nChanges not staged for commit:\n\tmodified:   z.txt\n\n\
            no changes added to commit (use \"git add\" and/or \"git commit -a\")\n";
        const UNTRACKED: &str = "On branch main\nUntracked files:\n\tu.txt\n\n\
            nothing added to commit but untracked files present (use \"git add\" to track)\n";
        let (yes, no) = (|| true, || false);
        // The index is asked only for a run that stopped (exit 1).
        let unasked = || -> bool { panic!("asked for unmerged entries") };

        assert_eq!(pick_end(0, unasked, "[main 5d695ee] feature adds b\n", ""), PickEnd::Done);
        assert_eq!(pick_end(1, yes, CONFLICT_OUT, CONFLICT_ERR), PickEnd::Conflicts);
        // Conflicts win over the wording: a status summary in the output changes nothing.
        assert_eq!(pick_end(1, yes, CLEAN, ""), PickEnd::Conflicts);
        // An empty cherry-pick (stderr advice), whatever else the tree holds; an empty revert (status only).
        assert_eq!(pick_end(1, no, CLEAN, EMPTY_ERR), PickEnd::Empty);
        assert_eq!(pick_end(1, no, UNSTAGED, EMPTY_ERR), PickEnd::Empty);
        assert_eq!(pick_end(1, no, CLEAN, ""), PickEnd::Empty);
        assert_eq!(pick_end(1, no, UNSTAGED, ""), PickEnd::Empty);
        assert_eq!(pick_end(1, no, UNTRACKED, ""), PickEnd::Empty);
        // Stopped without unmerged entries (rerere staged the resolution), or a hook refused the commit.
        assert_eq!(pick_end(1, no, CONFLICT_OUT, CONFLICT_ERR), PickEnd::Failed);
        assert_eq!(pick_end(1, no, "", "commit-msg: no ticket id\n"), PickEnd::Failed);
        // Fatal errors exit 128.
        let in_the_way = "error: Your local changes to the following files would be overwritten by merge:\n\ta.txt\n\
            Please commit your changes or stash them before you merge.\nAborting\nfatal: cherry-pick failed\n";
        assert_eq!(pick_end(128, unasked, "", in_the_way), PickEnd::Failed);
        let no_mainline = "error: commit 59bc8bf is a merge but no -m option was given.\nfatal: revert failed\n";
        assert_eq!(pick_end(128, unasked, "", no_mainline), PickEnd::Failed);
        assert_eq!(pick_end(-1, unasked, "", ""), PickEnd::Failed);
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
    fn pick_commands() {
        assert_eq!((PickOp::CherryPick.command(), PickOp::CherryPick.pending_ref()), ("cherry-pick", "CHERRY_PICK_HEAD"));
        assert_eq!((PickOp::Revert.command(), PickOp::Revert.pending_ref()), ("revert", "REVERT_HEAD"));
    }

    #[test]
    fn oids_and_remotes() {
        assert!(check_oid("4b825dc642cb6eb9a060e54bf8d69288fbee4904").is_ok());
        assert!(check_oid("abcd").is_ok());
        assert!(check_oid("abc").is_err());
        assert!(check_oid("HEAD").is_err());
        assert!(check_oid("stash@{0}").is_err());
        assert!(check_remote("origin").is_ok());
        assert!(check_remote("team/upstream").is_ok());
        assert!(check_remote("").is_err());
        assert!(check_remote("--all").is_err());
        assert!(check_remote("a b").is_err());
    }
}
