//! Repository discovery and in-progress operation detection.

use crate::error::{AppError, AppResult};
use crate::git::cmd::{lossy, Git};
use crate::model::{RepoInfo, RepoOperation};
use std::cell::OnceCell;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

/// `git rev-parse` discovery output, paths as printed by git.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Discovery {
    pub git_dir: String,
    pub common_dir: String,
    pub bare: bool,
    /// None for bare repos and when git ran inside a git dir.
    pub toplevel: Option<String>,
}

/// Resolve any path inside a repository (or a bare repo dir) to its RepoInfo.
/// Canonicalizes paths (realpath) so the same repo always yields the same `path` id.
pub fn open(path: &Path) -> AppResult<RepoInfo> {
    let meta = std::fs::metadata(path).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => AppError::msg(format!("not found: {}", path.display())),
        _ => AppError::msg(format!("{}: {e}", path.display())),
    })?;
    let dir = if meta.is_dir() {
        path
    } else {
        path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."))
    };
    let out = Git::new(dir)
        .args([
            "rev-parse",
            "--path-format=absolute",
            "--git-dir",
            "--git-common-dir",
            "--is-bare-repository",
            "--show-toplevel",
        ])
        .output()?;
    // In a bare repo or inside a git dir `--show-toplevel` fails (exit 128) after the other lines.
    let Some(found) = parse_rev_parse(&out.stdout) else {
        let stderr = lossy(&out.stderr);
        if stderr.trim().is_empty() || stderr.contains("not a git repository") {
            return Err(AppError::msg("not a git repository"));
        }
        // e.g. "detected dubious ownership", "safe.bareRepository is 'explicit'".
        return Err(AppError::Git { args: "rev-parse".into(), code: Some(out.code), stderr });
    };
    let git_dir = canonical(&found.git_dir);
    let common_dir = canonical(&found.common_dir);
    let toplevel = match found.toplevel {
        Some(top) if !found.bare => Some(canonical(top)),
        // Inside a git dir. git calls the admin dir of a bare repo's linked worktree bare as well.
        _ if !found.bare || git_dir != common_dir => worktree_of_git_dir(Path::new(&git_dir)),
        _ => None,
    }
    // Every later command runs in `path` and must find this repo again: not the case for a work tree
    // reached only through `core.worktree` (e.g. yadm) or named by a missing/stale `gitdir` file.
    .filter(|top| discovers(Path::new(top), &git_dir));
    // A git dir whose work tree cannot be located (or used) is viewed like a bare repo.
    let is_bare = toplevel.is_none();
    let path = toplevel.unwrap_or_else(|| git_dir.clone());
    Ok(RepoInfo {
        name: repo_name(Path::new(&path), is_bare),
        is_linked_worktree: git_dir != common_dir,
        path,
        git_dir,
        common_dir,
        is_bare,
    })
}

/// Parse `rev-parse --git-dir --git-common-dir --is-bare-repository --show-toplevel` output.
/// None unless the first three lines are present.
pub fn parse_rev_parse(stdout: &[u8]) -> Option<Discovery> {
    let mut lines = stdout.split(|b| *b == b'\n');
    let git_dir = lines.next().filter(|l| !l.is_empty())?;
    let common_dir = lines.next().filter(|l| !l.is_empty())?;
    let bare = match lines.next()? {
        b"true" => true,
        b"false" => false,
        _ => return None,
    };
    let toplevel = lines.next().filter(|l| !l.is_empty()).map(lossy);
    Some(Discovery { git_dir: lossy(git_dir), common_dir: lossy(common_dir), bare, toplevel })
}

/// `realpath` of `path` as a string, or `path` unchanged when it cannot be resolved (e.g. missing).
pub fn canonical(path: impl AsRef<Path>) -> String {
    let path = path.as_ref();
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf()).to_string_lossy().into_owned()
}

/// Git dir named by the gitfile `<dir>/.git` (`gitdir: <path>`, relative to `dir` when relative, as
/// with `worktree.useRelativePaths`). None when `.git` is a directory or missing.
pub fn gitfile_target(dir: &Path) -> Option<PathBuf> {
    let text = std::fs::read(dir.join(".git")).ok()?;
    let target = lossy(&text);
    let target = target.strip_prefix("gitdir:")?.trim();
    (!target.is_empty()).then(|| dir.join(target))
}

/// Whether git started in `top` discovers `git_dir` (canonical): `top/.git` is that directory or a
/// gitfile naming it.
fn discovers(top: &Path, git_dir: &str) -> bool {
    let dot_git = top.join(".git");
    let found = if dot_git.is_dir() { Some(dot_git) } else { gitfile_target(top) };
    found.is_some_and(|dir| canonical(dir) == git_dir)
}

/// Candidate work tree of a git dir that git ran inside of (`open` checks it with `discovers`): the
/// parent of a `.git` dir, or the worktree named by a linked worktree's admin dir
/// (`<common>/worktrees/<id>/gitdir` → `<worktree>/.git`).
fn worktree_of_git_dir(git_dir: &Path) -> Option<String> {
    if git_dir.file_name() == Some(OsStr::new(".git")) {
        return git_dir.parent().map(canonical);
    }
    let link = std::fs::read(git_dir.join("gitdir")).ok()?;
    let link = lossy(&link);
    // Relative with `worktree.useRelativePaths`; `join` keeps an absolute path as is.
    let dot_git = git_dir.join(link.trim_end_matches(['\n', '\r']));
    dot_git.parent().map(canonical)
}

/// Display name: the last path component; bare repos drop a trailing `.git` (`/x/.git` → `x`).
fn repo_name(path: &Path, bare: bool) -> String {
    let base = path.file_name().map_or_else(|| path.to_string_lossy(), |n| n.to_string_lossy()).into_owned();
    if bare {
        if let Some(stem) = base.strip_suffix(".git") {
            if !stem.is_empty() {
                return stem.to_string();
            }
            if let Some(parent) = path.parent().and_then(Path::file_name) {
                return parent.to_string_lossy().into_owned();
            }
        }
    }
    base
}

/// Detect merge/rebase/cherry-pick/revert/bisect/am in progress from files in a worktree's git dir.
/// The reftable backend keeps `CHERRY_PICK_HEAD`/`REVERT_HEAD` in its ref store instead of files; git
/// writes `MERGE_MSG` before either, so only then (and only for reftable) is git asked, once.
pub fn operation(git_dir: &Path) -> Option<RepoOperation> {
    let stored = OnceCell::new();
    let in_ref_store = |name: &str| {
        let [pick, revert] = *stored.get_or_init(|| {
            if git_dir.join("reftable").is_dir() && git_dir.join("MERGE_MSG").is_file() {
                sequencer_refs(git_dir)
            } else {
                [false; 2]
            }
        });
        (name == "CHERRY_PICK_HEAD" && pick) || (name == "REVERT_HEAD" && revert)
    };
    detect_operation(
        |rel| git_dir.join(rel).exists() || (matches!(rel, "CHERRY_PICK_HEAD" | "REVERT_HEAD") && in_ref_store(rel)),
        |rel| std::fs::read(git_dir.join(rel)).ok().map(|b| lossy(&b)),
    )
}

/// Whether `CHERRY_PICK_HEAD` and `REVERT_HEAD` resolve for the worktree of `git_dir` (one git call;
/// run inside the git dir, git resolves that worktree's own refs).
fn sequencer_refs(git_dir: &Path) -> [bool; 2] {
    Git::new(git_dir)
        .args(["cat-file", "--batch-check=%(objectname)"])
        .stdin(b"CHERRY_PICK_HEAD\nREVERT_HEAD\n".to_vec())
        .run()
        .map_or([false; 2], |out| parse_batch_check_found(&out))
}

/// `cat-file --batch-check=%(objectname)` output for two names: one line each, the oid when it
/// resolves, else `<name> missing`.
fn parse_batch_check_found(out: &[u8]) -> [bool; 2] {
    let mut lines = out.split(|b| *b == b'\n');
    let mut resolved = || lines.next().is_some_and(|l| !l.is_empty() && l.iter().all(u8::is_ascii_hexdigit));
    [resolved(), resolved()]
}

/// `operation` over abstract file access (`exists`/`read` take paths relative to the git dir).
/// First match wins: `rebase-merge/`, `rebase-apply/` (`am` when `applying`), `MERGE_HEAD`,
/// `CHERRY_PICK_HEAD`, `REVERT_HEAD`, `BISECT_LOG`. A rebase may leave `CHERRY_PICK_HEAD` behind,
/// so the rebase dirs are checked first.
pub fn detect_operation(exists: impl Fn(&str) -> bool, read: impl Fn(&str) -> Option<String>) -> Option<RepoOperation> {
    let number = |rel: &str| read(rel).and_then(|s| s.trim().parse::<u32>().ok());
    let branch = |rel: &str| read(rel).and_then(|s| branch_from_head_name(&s));
    let first_line = |rel: &str| {
        read(rel).and_then(|s| s.lines().next().map(str::trim).filter(|l| !l.is_empty()).map(str::to_string))
    };
    let op = |kind: &str, step: Option<u32>, total: Option<u32>, detail: Option<String>| {
        Some(RepoOperation { kind: kind.to_string(), step, total, detail })
    };
    if exists("rebase-merge") {
        op("rebase", number("rebase-merge/msgnum"), number("rebase-merge/end"), branch("rebase-merge/head-name"))
    } else if exists("rebase-apply") {
        let kind = if exists("rebase-apply/applying") { "am" } else { "rebase" };
        op(kind, number("rebase-apply/next"), number("rebase-apply/last"), branch("rebase-apply/head-name"))
    } else if exists("MERGE_HEAD") {
        op("merge", None, None, first_line("MERGE_MSG"))
    } else if exists("CHERRY_PICK_HEAD") {
        op("cherry-pick", None, None, first_line("MERGE_MSG"))
    } else if exists("REVERT_HEAD") {
        op("revert", None, None, first_line("MERGE_MSG"))
    } else if exists("BISECT_LOG") {
        op("bisect", None, None, None)
    } else {
        None
    }
}

/// Short branch name from a `head-name` or `BISECT_START` file (`refs/heads/x` or `x`); None when
/// it records a detached HEAD (`detached HEAD` or an oid).
pub fn branch_from_head_name(text: &str) -> Option<String> {
    let s = text.trim();
    let is_oid = (s.len() == 40 || s.len() == 64) && s.bytes().all(|b| b.is_ascii_hexdigit());
    if s.is_empty() || s == "detached HEAD" || is_oid {
        return None;
    }
    Some(s.strip_prefix("refs/heads/").unwrap_or(s).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn rev_parse_work_tree() {
        let d = parse_rev_parse(b"/p/repo/.git\n/p/repo/.git\nfalse\n/p/repo\n").unwrap();
        assert_eq!(
            d,
            Discovery {
                git_dir: "/p/repo/.git".into(),
                common_dir: "/p/repo/.git".into(),
                bare: false,
                toplevel: Some("/p/repo".into()),
            }
        );
    }

    #[test]
    fn rev_parse_linked_worktree_with_space() {
        let d = parse_rev_parse(b"/p/repo/.git/worktrees/wt-detached\n/p/repo/.git\nfalse\n/p/wt detached\n").unwrap();
        assert_eq!(d.git_dir, "/p/repo/.git/worktrees/wt-detached");
        assert_eq!(d.common_dir, "/p/repo/.git");
        assert_eq!(d.toplevel.as_deref(), Some("/p/wt detached"));
    }

    #[test]
    fn rev_parse_bare_and_inside_git_dir() {
        // Exit 128 ("this operation must be run in a work tree") after three lines.
        let bare = parse_rev_parse(b"/p/bare.git\n/p/bare.git\ntrue\n").unwrap();
        assert!(bare.bare);
        assert_eq!(bare.toplevel, None);
        let inside = parse_rev_parse(b"/p/repo/.git\n/p/repo/.git\nfalse\n").unwrap();
        assert!(!inside.bare);
        assert_eq!(inside.toplevel, None);
    }

    #[test]
    fn rev_parse_not_a_repo() {
        assert_eq!(parse_rev_parse(b""), None);
        assert_eq!(parse_rev_parse(b"/p/repo/.git\n/p/repo/.git\n"), None);
        assert_eq!(parse_rev_parse(b"--path-format=absolute\n/p/.git\n/p/.git\nfalse\n"), None);
    }

    #[test]
    fn names() {
        assert_eq!(repo_name(Path::new("/p/repo"), false), "repo");
        assert_eq!(repo_name(Path::new("/p/origin.git"), true), "origin");
        assert_eq!(repo_name(Path::new("/p/proj/.git"), true), "proj");
        assert_eq!(repo_name(Path::new("/p/plain"), true), "plain");
        assert_eq!(repo_name(Path::new("/p/x.git"), false), "x.git");
    }

    #[test]
    fn head_names() {
        assert_eq!(branch_from_head_name("refs/heads/feature/x\n").as_deref(), Some("feature/x"));
        assert_eq!(branch_from_head_name("main\n").as_deref(), Some("main"));
        assert_eq!(branch_from_head_name("detached HEAD\n"), None);
        assert_eq!(branch_from_head_name("7b46b4a51a02ab3cf0e21ec9838e3c40366a4560\n"), None);
        assert_eq!(branch_from_head_name(""), None);
    }

    fn detect(files: &[(&str, &str)]) -> Option<RepoOperation> {
        let map: HashMap<&str, &str> = files.iter().copied().collect();
        detect_operation(
            |rel| map.contains_key(rel) || map.keys().any(|k| k.starts_with(&format!("{rel}/"))),
            |rel| map.get(rel).map(|s| s.to_string()),
        )
    }

    #[test]
    fn rebase_merge_has_priority_over_leftover_cherry_pick_head() {
        let op = detect(&[
            ("rebase-merge/msgnum", "1\n"),
            ("rebase-merge/end", "3\n"),
            ("rebase-merge/head-name", "refs/heads/side\n"),
            ("CHERRY_PICK_HEAD", "1e0997d\n"),
            ("MERGE_MSG", "side change\n"),
        ])
        .unwrap();
        assert_eq!(op.kind, "rebase");
        assert_eq!((op.step, op.total), (Some(1), Some(3)));
        assert_eq!(op.detail.as_deref(), Some("side"));
    }

    #[test]
    fn rebase_of_detached_head_has_no_detail() {
        let op = detect(&[
            ("rebase-merge/msgnum", "2\n"),
            ("rebase-merge/end", "4\n"),
            ("rebase-merge/head-name", "detached HEAD\n"),
        ])
        .unwrap();
        assert_eq!((op.kind.as_str(), op.step, op.total, op.detail), ("rebase", Some(2), Some(4), None));
    }

    #[test]
    fn rebase_apply_and_am() {
        let rebase = detect(&[
            ("rebase-apply/next", "1\n"),
            ("rebase-apply/last", "3\n"),
            ("rebase-apply/head-name", "refs/heads/side\n"),
            ("rebase-apply/rebasing", ""),
        ])
        .unwrap();
        assert_eq!((rebase.kind.as_str(), rebase.step, rebase.total), ("rebase", Some(1), Some(3)));
        assert_eq!(rebase.detail.as_deref(), Some("side"));
        let am = detect(&[("rebase-apply/next", "2\n"), ("rebase-apply/last", "5\n"), ("rebase-apply/applying", "")])
            .unwrap();
        assert_eq!((am.kind.as_str(), am.step, am.total, am.detail), ("am", Some(2), Some(5), None));
    }

    #[test]
    fn merge_cherry_pick_revert_bisect() {
        let merge =
            detect(&[("MERGE_HEAD", "abc\n"), ("MERGE_MSG", "Merge branch 'side'\n\n# Conflicts:\n#\tf\n")]).unwrap();
        assert_eq!((merge.kind.as_str(), merge.detail.as_deref()), ("merge", Some("Merge branch 'side'")));
        assert_eq!((merge.step, merge.total), (None, None));
        let pick = detect(&[("CHERRY_PICK_HEAD", "abc\n"), ("MERGE_MSG", "side change\n")]).unwrap();
        assert_eq!((pick.kind.as_str(), pick.detail.as_deref()), ("cherry-pick", Some("side change")));
        let revert = detect(&[
            ("REVERT_HEAD", "abc\n"),
            ("MERGE_MSG", "Revert \"main change\"\n\nThis reverts commit 0d8e193.\n"),
        ])
        .unwrap();
        assert_eq!((revert.kind.as_str(), revert.detail.as_deref()), ("revert", Some("Revert \"main change\"")));
        let bisect = detect(&[("BISECT_LOG", "git bisect start\n"), ("BISECT_START", "main\n")]).unwrap();
        assert_eq!((bisect.kind.as_str(), bisect.detail), ("bisect", None));
        let merge_without_msg = detect(&[("MERGE_HEAD", "abc\n")]).unwrap();
        assert_eq!(merge_without_msg.detail, None);
    }

    #[test]
    fn reftable_sequencer_refs() {
        // Real output inside a reftable git dir during a cherry-pick, and with nothing in progress.
        assert_eq!(
            parse_batch_check_found(b"56e2fba1a2373e6bae2a00b73e943f4a4e08229c\nREVERT_HEAD missing\n"),
            [true, false]
        );
        assert_eq!(parse_batch_check_found(b"CHERRY_PICK_HEAD missing\nREVERT_HEAD missing\n"), [false, false]);
        assert_eq!(
            parse_batch_check_found(b"CHERRY_PICK_HEAD missing\n56e2fba1a2373e6bae2a00b73e943f4a4e08229c\n"),
            [false, true]
        );
        assert_eq!(parse_batch_check_found(b""), [false, false]);
    }

    #[test]
    fn no_operation() {
        assert!(detect(&[("HEAD", "ref: refs/heads/main\n"), ("ORIG_HEAD", "abc\n"), ("SQUASH_MSG", "x")]).is_none());
    }
}
