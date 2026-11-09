//! The few write operations the viewer offers: stage / unstage, commit, branch creation, stash and
//! the network trio fetch / pull / push. Every other command stays read-only. Each function runs one
//! git process (two for the stash and branch checks) through `cmd::Git`, so the usual env and config
//! overrides apply; the index lock is a required lock, which `--no-optional-locks` leaves alone.

use crate::error::{AppError, AppResult};
use crate::git::cmd::{lossy, Git, GitOutput};
use crate::model::OpResult;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

/// Network operations are killed after this long.
const NET_TIMEOUT: Duration = Duration::from_secs(600);
/// How long the login shell may take to print its PATH.
const SHELL_TIMEOUT: Duration = Duration::from_secs(10);

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

/// Git's branch name rules (`git check-ref-format --branch`): not empty, not starting with `-`, no
/// component starting with `.` or ending in `.lock`, no `..`, `@{` or `//`, not `@`, no ASCII control
/// characters, space or `~ ^ : ? * [ \`, no leading or trailing `/`, not ending with `.`.
pub fn valid_branch_name(name: &str) -> bool {
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
    if valid_branch_name(name) {
        Ok(())
    } else {
        Err(AppError::msg(format!("invalid branch name: {name}")))
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

// ───────────────────────────── environment ─────────────────────────────

static LOGIN_PATH: OnceLock<Option<String>> = OnceLock::new();

/// PATH of the user's login shell followed by the current PATH, resolved once (about 0.2 s). A GUI
/// app launched from the Finder runs with `/usr/bin:/bin:/usr/sbin:/sbin`, so commit hooks and
/// credential helpers configured by their bare name (`!gh …`, `node`) would not resolve. Besides
/// `cmd.rs` this is the one place that spawns a process, and it never spawns git: `$SHELL` (else
/// `/bin/zsh`) with `-ilc` printing `$PATH` between markers, 10 s deadline. None when that fails.
pub fn login_path() -> Option<&'static str> {
    LOGIN_PATH.get_or_init(resolve_login_path).as_deref()
}

fn resolve_login_path() -> Option<String> {
    let shell = std::env::var("SHELL").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| "/bin/zsh".into());
    let mut child = Command::new(shell)
        .args(["-ilc", "printf '__GV_PATH__%s__GV_END__' \"$PATH\""])
        .env("TERM", "dumb")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut stdout = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut v = Vec::new();
        let _ = std::io::Read::read_to_end(&mut stdout, &mut v);
        v
    });
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {}
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
        if started.elapsed() >= SHELL_TIMEOUT {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let out = reader.join().ok()?;
    let text = lossy(&out);
    let start = text.find("__GV_PATH__")? + "__GV_PATH__".len();
    let end = text[start..].find("__GV_END__")? + start;
    let path = text[start..end].trim();
    if path.is_empty() {
        return None;
    }
    let current = std::env::var("PATH").unwrap_or_default();
    Some(if current.is_empty() { path.to_string() } else { format!("{path}:{current}") })
}

/// The login-shell PATH on a git that runs hooks or helpers.
fn with_login_path(git: Git) -> Git {
    match login_path() {
        Some(path) => git.env("PATH", path),
        None => git,
    }
}

/// A network operation: no process slot (it may run for minutes), a deadline, killed when the repo's
/// last tab closes, hooks and helpers on the login-shell PATH.
fn net(git: Git, stop: Arc<AtomicBool>) -> Git {
    with_login_path(git.unslotted().timeout(NET_TIMEOUT).cancel_on(stop))
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
/// ever opens; hooks run on the login-shell PATH.
pub fn commit(cwd: &Path, message: &str, amend: bool) -> AppResult<OpResult> {
    if message.trim().is_empty() {
        return Err(AppError::msg("empty commit message"));
    }
    let git = Git::new(cwd)
        .args(["commit", "-q", "-F", "-"])
        .args(amend.then_some("--amend"))
        .stdin(message.as_bytes().to_vec())
        .env("GIT_EDITOR", "true");
    with_login_path(git).output_checked().map(result)
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

// ───────────────────────────── network ─────────────────────────────

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
    fn branch_names() {
        for ok in ["main", "feature/x", "a.b", "v1.0", "a/b/c", "fix-1_2", "한글"] {
            assert!(valid_branch_name(ok), "{ok}");
        }
        for bad in [
            "", "-x", ".hidden", "a..b", "a/", "/a", "x.lock", "a b", "a~b", "a^b", "a:b", "a?b", "a*b", "a[b", "a\\b",
            "@", "a@{b", "a//b", "a.", "dir/.x", "a\tb", "a\u{7f}b", "dir/x.lock/y",
        ] {
            assert!(!valid_branch_name(bad), "{bad:?}");
        }
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
