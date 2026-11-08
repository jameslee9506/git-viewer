//! Commit details and changed-file lists.
//!
//! Also hosts the input validation and HEAD/empty-tree helpers shared with `diff` and `blob`.

use crate::error::{AppError, AppResult};
use crate::git::cmd::{lossy, split_records, Git};
use crate::model::{CommitDetails, DiffTarget, FileChange, Person};
use std::collections::HashSet;
use std::path::{Component, Path};

const DETAILS_FORMAT: &str = "--format=%H%x00%P%x00%aN%x00%aE%x00%ad%x00%cN%x00%cE%x00%cd%x00%B";

/// Message, people and files (vs first parent; root commit vs empty tree).
/// The header and the file list are read by two concurrent processes.
pub fn details(cwd: &Path, oid: &str) -> AppResult<CommitDetails> {
    validate_oid(oid)?;
    let (out, files) = std::thread::scope(|s| {
        let files = s.spawn(|| first_parent_files(cwd, oid));
        let out = Git::new(cwd).args(["log", "-1", "-z", "--date=raw", DETAILS_FORMAT, oid, "--"]).run();
        (out, files.join().unwrap_or_else(|_| Err(AppError::msg("reading changed files failed"))))
    });
    let mut d = parse_commit_details(&out?).ok_or_else(|| AppError::msg(format!("commit not found: {oid}")))?;
    d.files = files?;
    let (additions, deletions) = totals(&d.files);
    d.additions = additions;
    d.deletions = deletions;
    Ok(d)
}

/// Changed files between two trees (`from` None = root commit / empty tree).
pub fn changed_files(cwd: &Path, from: Option<&str>, to: &str) -> AppResult<Vec<FileChange>> {
    validate_rev(to)?;
    match from {
        Some(from) => {
            validate_rev(from)?;
            tree_files(cwd, &[from, to])
        }
        None => tree_files(cwd, &["--root", "--no-commit-id", to]),
    }
}

/// Changed files for any diff target (commit, range, stash, stashUntracked, staged, unstaged, untracked,
/// worktree).
///
/// Staged/unstaged lists leave out unmerged paths (they belong to the conflicted list, which is
/// derived from the working status, so `Conflicted` returns an empty list). The worktree list keeps
/// them: without `--cached`, `diff-index` compares an unmerged path's file on disk (never a `U` entry).
pub fn diff_files(repo: &Path, target: &DiffTarget) -> AppResult<Vec<FileChange>> {
    match target {
        DiffTarget::Commit { oid, parent } => {
            validate_oid(oid)?;
            match parent {
                Some(p) => {
                    validate_oid(p)?;
                    changed_files(repo, Some(p), oid)
                }
                None => first_parent_files(repo, oid),
            }
        }
        DiffTarget::Range { from, to } => changed_files(repo, Some(from), to),
        DiffTarget::Stash { oid } => {
            validate_oid(oid)?;
            changed_files(repo, Some(&format!("{oid}^1")), oid)
        }
        DiffTarget::StashUntracked { oid } => {
            validate_oid(oid)?;
            let mut files = changed_files(repo, None, &format!("{oid}^3"))?;
            for f in &mut files {
                f.status = "?".into();
            }
            Ok(files)
        }
        DiffTarget::Staged { cwd } => {
            let cwd = worktree_dir(cwd)?;
            let out = with_head_tree(cwd, |base| {
                Git::new(cwd).args(["diff-index", "--cached", "-z", "-M", "--raw", "--numstat", base, "--"]).run()
            })?;
            Ok(without_unmerged(parse_raw_numstat(&out)))
        }
        DiffTarget::Unstaged { cwd } => {
            let cwd = worktree_dir(cwd)?;
            // `-M` pairs a deleted file with an intent-to-add file like status does (`2 .R`).
            let out = Git::new(cwd).args(["diff-files", "-z", "-M", "--raw", "--numstat", "--"]).run()?;
            Ok(without_unmerged(without_stat_only(parse_raw_numstat(&out))))
        }
        DiffTarget::Conflicted { .. } => Ok(Vec::new()),
        DiffTarget::Worktree { cwd, oid } => {
            validate_oid(oid)?;
            let cwd = worktree_dir(cwd)?;
            // Plumbing like the staged/unstaged lists: porcelain `git diff` would refresh `.git/index`.
            let out = Git::new(cwd).args(["diff-index", "-z", "-M", "--raw", "--numstat", oid.as_str(), "--"]).run()?;
            Ok(without_stat_only(parse_raw_numstat(&out)))
        }
        DiffTarget::Untracked { cwd } => {
            let cwd = worktree_dir(cwd)?;
            let out = Git::new(cwd).args(["ls-files", "--others", "--exclude-standard", "-z", "--"]).run()?;
            Ok(parse_untracked(&out))
        }
    }
}

/// Parse the `git log -1 -z --date=raw` output of `details` (files and totals left empty).
pub fn parse_commit_details(buf: &[u8]) -> Option<CommitDetails> {
    let buf = buf.strip_suffix(b"\0").unwrap_or(buf);
    let mut f = buf.splitn(9, |b| *b == 0);
    let oid = lossy(f.next()?);
    if oid.is_empty() {
        return None;
    }
    let parents = lossy(f.next()?).split_whitespace().map(str::to_string).collect();
    let author = person(f.next()?, f.next()?, f.next()?);
    let committer = person(f.next()?, f.next()?, f.next()?);
    let (subject, body) = split_message(&lossy(f.next().unwrap_or_default()));
    Some(CommitDetails {
        oid,
        parents,
        author,
        committer,
        subject,
        body,
        files: Vec::new(),
        additions: 0,
        deletions: 0,
    })
}

/// Parse `git diff-tree -r -z -M --raw --numstat` output (also `diff-index`/`diff-files` with the same flags).
///
/// Raw entries (`:<sm> <dm> <so> <do> <STATUS[score]>\0path\0[newpath\0]`) carry status, modes and paths;
/// numstat entries (`add\tdel\tpath\0` or `add\tdel\t\0old\0new\0`, `-\t-` = binary) are paired with them
/// in order, falling back to the next unpaired entry with the same paths (entries without one keep `None`
/// counts). Unrecognized tokens (such as a leading commit id) are skipped.
pub fn parse_raw_numstat(buf: &[u8]) -> Vec<FileChange> {
    let toks = split_records(buf, 0);
    let mut files: Vec<FileChange> = Vec::new();
    let mut paired: Vec<bool> = Vec::new();
    let mut cursor = 0;
    let mut i = 0;
    while i < toks.len() {
        let tok = toks[i];
        i += 1;
        if tok.first() == Some(&b':') {
            let Some(raw) = parse_raw_header(tok) else { continue };
            let two_paths = matches!(raw.status.as_bytes().first(), Some(b'R' | b'C'));
            let first = toks.get(i).copied();
            let second = if two_paths { toks.get(i + 1).copied() } else { None };
            i += if two_paths { 2 } else { 1 };
            let (old_path, path) = match (first, second, two_paths) {
                (Some(old), Some(new), true) => (Some(lossy(old)), lossy(new)),
                (Some(p), _, false) => (None, lossy(p)),
                _ => break,
            };
            let similarity = if two_paths { raw.score } else { None };
            let submodule = raw.old_mode == "160000" || raw.new_mode == "160000";
            files.push(FileChange {
                path,
                old_path,
                status: raw.status,
                additions: None,
                deletions: None,
                binary: false,
                similarity,
                old_mode: non_zero_mode(raw.old_mode),
                new_mode: non_zero_mode(raw.new_mode),
                submodule,
            });
            paired.push(false);
        } else if let Some((add, del, rest)) = split_numstat(tok) {
            let (old_path, path) = if rest.is_empty() {
                let (Some(old), Some(new)) = (toks.get(i), toks.get(i + 1)) else { break };
                i += 2;
                (Some(lossy(old)), lossy(new))
            } else {
                (None, lossy(rest))
            };
            let matches = |f: &FileChange| f.path == path && (old_path.is_none() || f.old_path == old_path);
            let found =
                (cursor..files.len()).chain(0..cursor.min(files.len())).find(|&k| !paired[k] && matches(&files[k]));
            if let Some(k) = found {
                let f = &mut files[k];
                f.binary = add == b"-" || del == b"-";
                f.additions = parse_count(add);
                f.deletions = parse_count(del);
                paired[k] = true;
                cursor = k + 1;
            }
        }
    }
    files
}

/// Parse `git ls-files --others --exclude-standard -z` output into untracked entries (`?`, no counts).
/// Untracked nested repositories keep their trailing `/`.
pub fn parse_untracked(buf: &[u8]) -> Vec<FileChange> {
    split_records(buf, 0)
        .into_iter()
        .filter(|p| !p.is_empty())
        .map(|p| FileChange {
            path: lossy(p),
            old_path: None,
            status: "?".into(),
            additions: None,
            deletions: None,
            binary: false,
            similarity: None,
            old_mode: None,
            new_mode: None,
            submodule: false,
        })
        .collect()
}

/// Changes of `oid` against its first parent (root commit: against the empty tree) in one process,
/// without resolving the parents first.
fn first_parent_files(cwd: &Path, oid: &str) -> AppResult<Vec<FileChange>> {
    tree_files(cwd, &["--root", "--no-commit-id", "--diff-merges=first-parent", oid])
}

/// `git diff-tree -r -z --raw -M --numstat <args> --`. Without local blobs (partial clone, lazy fetching
/// disabled) numstat and rename detection fail; then the plain raw list is returned (no counts, no renames).
fn tree_files(cwd: &Path, args: &[&str]) -> AppResult<Vec<FileChange>> {
    let run =
        |extra: &[&str]| Git::new(cwd).args(["diff-tree", "-r", "-z", "--raw"]).args(extra).args(args).arg("--").run();
    match run(&["-M", "--numstat"]) {
        Ok(out) => Ok(parse_raw_numstat(&out)),
        Err(e) => run(&[]).map(|out| parse_raw_numstat(&out)).map_err(|_| e),
    }
}

/// Drop modifications without a numstat entry, mode change or gitlink: `diff-files` and `diff-index`
/// without `--cached` report files whose stat data changed although the content did not (`git status` and
/// `git diff` hide them too).
fn without_stat_only(mut files: Vec<FileChange>) -> Vec<FileChange> {
    files.retain(|f| f.status != "M" || f.additions.is_some() || f.binary || f.old_mode != f.new_mode || f.submodule);
    files
}

/// Sums of the known line counts.
fn totals(files: &[FileChange]) -> (u32, u32) {
    files.iter().fold((0u32, 0u32), |(a, d), f| {
        (a.saturating_add(f.additions.unwrap_or(0)), d.saturating_add(f.deletions.unwrap_or(0)))
    })
}

/// Drop every entry of a path that has an unmerged (`U`) entry.
fn without_unmerged(files: Vec<FileChange>) -> Vec<FileChange> {
    let unmerged: HashSet<String> = files.iter().filter(|f| f.status == "U").map(|f| f.path.clone()).collect();
    if unmerged.is_empty() {
        return files;
    }
    files.into_iter().filter(|f| !unmerged.contains(&f.path)).collect()
}

struct RawHeader {
    old_mode: String,
    new_mode: String,
    status: String,
    score: Option<u32>,
}

/// `:<sm> <dm> <so> <do> <STATUS[score]>`.
fn parse_raw_header(tok: &[u8]) -> Option<RawHeader> {
    let s = lossy(&tok[1..]);
    let mut it = s.split(' ');
    let old_mode = it.next()?.to_string();
    let new_mode = it.next()?.to_string();
    let _old_oid = it.next()?;
    let _new_oid = it.next()?;
    let st = it.next()?;
    let mut chars = st.chars();
    let status = chars.next()?.to_string();
    let score = chars.as_str().parse().ok();
    Some(RawHeader { old_mode, new_mode, status, score })
}

/// `add\tdel\trest` with `add`/`del` digits or `-`; splits only on the first two TABs (paths may contain TAB).
fn split_numstat(tok: &[u8]) -> Option<(&[u8], &[u8], &[u8])> {
    let t1 = tok.iter().position(|b| *b == b'\t')?;
    let t2 = t1 + 1 + tok[t1 + 1..].iter().position(|b| *b == b'\t')?;
    let (add, del) = (&tok[..t1], &tok[t1 + 1..t2]);
    let is_count = |c: &[u8]| c == b"-" || (!c.is_empty() && c.iter().all(u8::is_ascii_digit));
    (is_count(add) && is_count(del)).then(|| (add, del, &tok[t2 + 1..]))
}

fn parse_count(c: &[u8]) -> Option<u32> {
    std::str::from_utf8(c).ok()?.parse().ok()
}

fn non_zero_mode(mode: String) -> Option<String> {
    (!mode.is_empty() && mode.bytes().any(|b| b != b'0')).then_some(mode)
}

fn person(name: &[u8], email: &[u8], date: &[u8]) -> Person {
    let date = lossy(date);
    let date = date.trim();
    let (time, tz) = date.split_once(' ').unwrap_or((date, ""));
    Person { name: lossy(name), email: lossy(email), time: time.parse().unwrap_or(0), tz: tz.trim().to_string() }
}

/// Subject = first non-blank line; body = the rest without leading blank lines and trailing whitespace
/// (indentation of the first body line is kept, CRLF normalized).
fn split_message(msg: &str) -> (String, String) {
    let msg = msg.trim_start();
    let (subject, mut body) = msg.split_once('\n').unwrap_or((msg, ""));
    while let Some((line, rest)) = body.split_once('\n') {
        if !line.trim().is_empty() {
            break;
        }
        body = rest;
    }
    (subject.trim().to_string(), body.trim_end().replace("\r\n", "\n"))
}

// ───────────────────────────── Shared by commit / diff / blob ─────────────────────────────

/// Accept only a 4–64 digit hex object id.
pub(crate) fn validate_oid(oid: &str) -> AppResult<()> {
    if (4..=64).contains(&oid.len()) && oid.bytes().all(|b| b.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err(AppError::msg(format!("invalid object id: {oid}")))
    }
}

/// Accept a revision that cannot be mistaken for an option.
pub(crate) fn validate_rev(rev: &str) -> AppResult<()> {
    if rev.is_empty() || rev.starts_with('-') || rev.contains('\0') {
        Err(AppError::msg(format!("invalid revision: {rev}")))
    } else {
        Ok(())
    }
}

/// Accept a non-empty path without NUL bytes. Paths are always passed after `--` or inside an object
/// name, so a leading `-` is harmless and allowed (files may be named like that).
pub(crate) fn validate_path(path: &str) -> AppResult<()> {
    if path.is_empty() || path.contains('\0') {
        Err(AppError::msg(format!("invalid path: {path}")))
    } else {
        Ok(())
    }
}

/// A repo-relative path that stays inside the worktree (no root, prefix or `..` components).
pub(crate) fn relative_path(path: &str) -> AppResult<&Path> {
    validate_path(path)?;
    let p = Path::new(path);
    if p.components().all(|c| matches!(c, Component::Normal(_) | Component::CurDir)) {
        Ok(p)
    } else {
        Err(AppError::msg(format!("invalid path: {path}")))
    }
}

/// An existing absolute directory to run worktree commands in.
pub(crate) fn worktree_dir(cwd: &str) -> AppResult<&Path> {
    let p = Path::new(cwd);
    if p.is_absolute() && p.is_dir() {
        Ok(p)
    } else {
        Err(AppError::msg(format!("not a directory: {cwd}")))
    }
}

/// Run `f` with `HEAD` as the base tree; when that fails because HEAD is unborn, run it again with the
/// empty tree of the repository's object format.
pub(crate) fn with_head_tree<T>(cwd: &Path, f: impl Fn(&str) -> AppResult<T>) -> AppResult<T> {
    let err = match f("HEAD") {
        Ok(v) => return Ok(v),
        Err(e) => e,
    };
    let head = Git::new(cwd).args(["rev-parse", "-q", "--verify", "HEAD"]).output()?;
    if head.code != 1 {
        return Err(err);
    }
    let empty_tree = Git::new(cwd).args(["hash-object", "-t", "tree", "/dev/null"]).run_string()?;
    f(empty_tree.trim())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn by_path<'a>(files: &'a [FileChange], path: &str) -> &'a FileChange {
        files.iter().find(|f| f.path == path).unwrap_or_else(|| panic!("missing {path}"))
    }

    fn counts(f: &FileChange) -> (Option<u32>, Option<u32>) {
        (f.additions, f.deletions)
    }

    const DETAILS_ROOT: &[u8] = b"0205c3f1b4884b20e95e8688230b654e76cd0067\0\0Ann Author\0ann@example.com\0\
        1700003600 +0900\0Cal Committer\0cal@example.com\x001700003600 -0130\0root commit\n\
        \n\
        Body line 1\n\
        Body line 2\n\
        \n\
        Signed-off-by: Ann Author <ann@example.com>\n\
        \0";

    const DETAILS_MERGE: &[u8] =
        b"b5df119ec72ff8a2785e874753582f7019d813de\x0098f0816764393ec835690a6cf787b29b31234a73 bfa833f9c85c35482d64acc7f227bd7ceb8e1d42\0\
        Ann Author\0ann@example.com\x001700025200 +0900\0Cal Committer\0cal@example.com\0\
        1700025200 -0130\0Merge branch 'side'\n\
        \0";

    // `git diff-tree -r -z -M --raw --numstat --root --no-commit-id <root>`
    const RAW_ROOT: &[u8] =
        b":000000 100644 0000000000000000000000000000000000000000 8178c76d627cade75005b40711b92f4177bc6cfc A\0\
        README.md\0:000000 100644 0000000000000000000000000000000000000000 83db48f84ec878fbfb30b46d16630e944e34f205 A\0\
        a.txt\0:000000 100644 0000000000000000000000000000000000000000 83536cff1521d812a0feee584bd568ea709389f8 A\0\
        bin.dat\0:000000 100644 0000000000000000000000000000000000000000 627165f5e27b6486daf3678a4edf4688de9aa47c A\0\
        copy-src.txt\0:000000 100644 0000000000000000000000000000000000000000 75f18be4fd2a229531097e94374316da64baa0f2 A\0\
        crlf.txt\0:000000 100644 0000000000000000000000000000000000000000 06cda4b7b93b86b74b1f1fc9b676a8f7936bc736 A\0\
        dashes.txt\0:000000 100644 0000000000000000000000000000000000000000 ce013625030ba8dba906f756967f9e9ca394464a A\0\
        dir with space/file name.txt\0:000000 100644 0000000000000000000000000000000000000000 e69de29bb2d1d6434b8b29ae775ad8c2e48c5391 A\0\
        empty.txt\0:000000 120000 0000000000000000000000000000000000000000 8d14cbf983b3fad683171c9418998d9f68340823 A\0\
        link\0:000000 100644 0000000000000000000000000000000000000000 35be908f97592ab48b79b15942eba5aa7959aab9 A\0\
        long.txt\0:000000 100644 0000000000000000000000000000000000000000 96bda30e0c2cef9a2f01fe00388a3c42180ba71a A\0\
        nonl-new.txt\0:000000 100644 0000000000000000000000000000000000000000 31cab939b11e973f3b3a88905fe7c5066c5dc46d A\0\
        nonl-old.txt\0:000000 100644 0000000000000000000000000000000000000000 daf3c426ac9136633240049e259e1e3f6d7dc26c A\0\
        nonl.txt\0:000000 100644 0000000000000000000000000000000000000000 4163036efa65bd4a469e752267498f01ea36a55c A\0\
        script.sh\0:000000 100644 0000000000000000000000000000000000000000 7a44627979a55254e1f25e2bdfcfd8b269868610 A\0\
        \xed\x95\x9c\xea\xb8\x80 \xed\x8c\x8c\xec\x9d\xbc.txt\x001\t0\tREADME.md\0\
        3\t0\ta.txt\0-\t-\tbin.dat\x0030\t0\tcopy-src.txt\x003\t0\tcrlf.txt\x004\t0\tdashes.txt\0\
        1\t0\tdir with space/file name.txt\x000\t0\tempty.txt\x001\t0\tlink\x0040\t0\tlong.txt\0\
        2\t0\tnonl-new.txt\x002\t0\tnonl-old.txt\x002\t0\tnonl.txt\x002\t0\tscript.sh\0\
        1\t0\t\xed\x95\x9c\xea\xb8\x80 \xed\x8c\x8c\xec\x9d\xbc.txt\0";

    // Rename with edits, binary, mode change, delete, empty file, typechange, unicode (vs the root commit).
    const RAW_EDITS: &[u8] =
        b":100644 000000 8178c76d627cade75005b40711b92f4177bc6cfc 0000000000000000000000000000000000000000 D\0\
        README.md\0:100644 100644 83536cff1521d812a0feee584bd568ea709389f8 8e498a275ff4d37b96e6a6496e6ef26af35109d4 M\0\
        bin.dat\0:000000 100644 0000000000000000000000000000000000000000 627165f5e27b6486daf3678a4edf4688de9aa47c A\0\
        copy-dst.txt\0:100644 100644 627165f5e27b6486daf3678a4edf4688de9aa47c 6774ffc32a1dfda3bb15bdc01cd082d3e6c79e3f M\0\
        copy-src.txt\0:100644 100644 75f18be4fd2a229531097e94374316da64baa0f2 4b63a83b9df0e310d96c3ce18f108358874aa0a8 M\0\
        crlf.txt\0:100644 100644 06cda4b7b93b86b74b1f1fc9b676a8f7936bc736 9880d0735333103ce1a0c913e1560cfd219f106f M\0\
        dashes.txt\0:000000 100644 0000000000000000000000000000000000000000 e69de29bb2d1d6434b8b29ae775ad8c2e48c5391 A\0\
        empty2.txt\0:120000 100644 8d14cbf983b3fad683171c9418998d9f68340823 3f899ea7ab51da801dbacbf633c168b0591d7765 T\0\
        link\0:100644 100644 96bda30e0c2cef9a2f01fe00388a3c42180ba71a 5d9b138163b966315b60458d835cfbb3b3d77b63 M\0\
        nonl-new.txt\0:100644 100644 31cab939b11e973f3b3a88905fe7c5066c5dc46d 11f660feae9dd8551c364f0da9bac0483e94099c M\0\
        nonl-old.txt\0:100644 100644 daf3c426ac9136633240049e259e1e3f6d7dc26c f9f7b69167a2970e1eecc6f2bc11cb37e9c715c1 M\0\
        nonl.txt\0:100644 100644 35be908f97592ab48b79b15942eba5aa7959aab9 afdb0f7f895ef388c1c50e58a8ac4e73bc561458 R096\0\
        long.txt\0renamed long.txt\0:100644 100755 4163036efa65bd4a469e752267498f01ea36a55c 4163036efa65bd4a469e752267498f01ea36a55c M\0\
        script.sh\0:100644 100644 7a44627979a55254e1f25e2bdfcfd8b269868610 d6bbe256e5c8d78a973aeff07d955365b7ffb5b4 M\0\
        \xed\x95\x9c\xea\xb8\x80 \xed\x8c\x8c\xec\x9d\xbc.txt\x000\t1\tREADME.md\0\
        -\t-\tbin.dat\x0030\t0\tcopy-dst.txt\x001\t0\tcopy-src.txt\x001\t1\tcrlf.txt\0\
        1\t1\tdashes.txt\x000\t0\tempty2.txt\x001\t1\tlink\x001\t1\tnonl-new.txt\x001\t1\tnonl-old.txt\0\
        1\t1\tnonl.txt\x001\t1\t\0long.txt\0renamed long.txt\x000\t0\tscript.sh\x001\t1\t\xed\x95\x9c\xea\xb8\x80 \xed\x8c\x8c\xec\x9d\xbc.txt\0";

    // Same commit with `-C`: copy entry (C100) whose source is also modified.
    const RAW_COPY: &[u8] =
        b":100644 000000 8178c76d627cade75005b40711b92f4177bc6cfc 0000000000000000000000000000000000000000 D\0\
        README.md\0:100644 100644 83536cff1521d812a0feee584bd568ea709389f8 8e498a275ff4d37b96e6a6496e6ef26af35109d4 M\0\
        bin.dat\0:100644 100644 627165f5e27b6486daf3678a4edf4688de9aa47c 627165f5e27b6486daf3678a4edf4688de9aa47c C100\0\
        copy-src.txt\0copy-dst.txt\0:100644 100644 627165f5e27b6486daf3678a4edf4688de9aa47c 6774ffc32a1dfda3bb15bdc01cd082d3e6c79e3f M\0\
        copy-src.txt\0:100644 100644 75f18be4fd2a229531097e94374316da64baa0f2 4b63a83b9df0e310d96c3ce18f108358874aa0a8 M\0\
        crlf.txt\0:100644 100644 06cda4b7b93b86b74b1f1fc9b676a8f7936bc736 9880d0735333103ce1a0c913e1560cfd219f106f M\0\
        dashes.txt\0:000000 100644 0000000000000000000000000000000000000000 e69de29bb2d1d6434b8b29ae775ad8c2e48c5391 A\0\
        empty2.txt\0:120000 100644 8d14cbf983b3fad683171c9418998d9f68340823 3f899ea7ab51da801dbacbf633c168b0591d7765 T\0\
        link\0:100644 100644 96bda30e0c2cef9a2f01fe00388a3c42180ba71a 5d9b138163b966315b60458d835cfbb3b3d77b63 M\0\
        nonl-new.txt\0:100644 100644 31cab939b11e973f3b3a88905fe7c5066c5dc46d 11f660feae9dd8551c364f0da9bac0483e94099c M\0\
        nonl-old.txt\0:100644 100644 daf3c426ac9136633240049e259e1e3f6d7dc26c f9f7b69167a2970e1eecc6f2bc11cb37e9c715c1 M\0\
        nonl.txt\0:100644 100644 35be908f97592ab48b79b15942eba5aa7959aab9 afdb0f7f895ef388c1c50e58a8ac4e73bc561458 R096\0\
        long.txt\0renamed long.txt\0:100644 100755 4163036efa65bd4a469e752267498f01ea36a55c 4163036efa65bd4a469e752267498f01ea36a55c M\0\
        script.sh\0:100644 100644 7a44627979a55254e1f25e2bdfcfd8b269868610 d6bbe256e5c8d78a973aeff07d955365b7ffb5b4 M\0\
        \xed\x95\x9c\xea\xb8\x80 \xed\x8c\x8c\xec\x9d\xbc.txt\x000\t1\tREADME.md\0\
        -\t-\tbin.dat\x000\t0\t\0copy-src.txt\0copy-dst.txt\x001\t0\tcopy-src.txt\0\
        1\t1\tcrlf.txt\x001\t1\tdashes.txt\x000\t0\tempty2.txt\x001\t1\tlink\x001\t1\tnonl-new.txt\0\
        1\t1\tnonl-old.txt\x001\t1\tnonl.txt\x001\t1\t\0long.txt\0renamed long.txt\0\
        0\t0\tscript.sh\x001\t1\t\xed\x95\x9c\xea\xb8\x80 \xed\x8c\x8c\xec\x9d\xbc.txt\0";

    const RAW_SUBMODULE: &[u8] =
        b":000000 100644 0000000000000000000000000000000000000000 90ed8eb13d1f2ce3e8f0fa0c3e046455013b0364 A\0\
        .gitmodules\0:000000 160000 0000000000000000000000000000000000000000 2d283f156df461aa10b92870abe12178e787e4fe A\0\
        libs/sub\x003\t0\t.gitmodules\x001\t0\tlibs/sub\0";

    const RAW_SUBMODULE_BUMP: &[u8] =
        b":160000 160000 2d283f156df461aa10b92870abe12178e787e4fe 6616a9a889347a2147e684f6f4d365b33259109a M\0\
        libs/sub\x001\t1\tlibs/sub\0";

    // Merge commit vs its first parent.
    const RAW_MERGE: &[u8] =
        b":100644 100644 4a1223fb7d818b89cbe1322552e38c207295f9ea df814d73085d7949feb11a96fb28593257959f31 M\0\
        a.txt\0:000000 100644 0000000000000000000000000000000000000000 bfe2583bd1bb8581d65683f569efe16c5f30eb03 A\0\
        side.txt\x001\t1\ta.txt\x001\t0\tside.txt\0";

    // `--root <stash>^3` without `--no-commit-id`: the commit id comes first.
    const RAW_STASH_UNTRACKED_WITH_ID: &[u8] =
        b"4b76372e6ddbfcd4afb339f13abd01fbb96e7bc5\0:000000 100644 0000000000000000000000000000000000000000 4ae8ef021bf6fcfff43a13be5abfa52bb6fb5dbc A\0\
        ud/inner.txt\0:000000 100644 0000000000000000000000000000000000000000 5a72eb2edc5d0da32ff615d210d6fa90c31ed940 A\0\
        untracked new.txt\x001\t0\tud/inner.txt\x001\t0\tuntracked new.txt\0";

    // `git diff-index --cached -z -M --raw --numstat HEAD`: staged modification, rename and new file.
    const RAW_STAGED: &[u8] =
        b":100644 100644 df814d73085d7949feb11a96fb28593257959f31 96aabf3ef4cbee8f175966e4f2f23e8fe2240045 M\0\
        a.txt\0:100644 100644 bfe2583bd1bb8581d65683f569efe16c5f30eb03 bfe2583bd1bb8581d65683f569efe16c5f30eb03 R100\0\
        side.txt\0side-renamed.txt\0:000000 100644 0000000000000000000000000000000000000000 91d552370eeb567aa1e93a74810ab2e7323295c6 A\0\
        staged-new.txt\x001\t0\ta.txt\x000\t0\t\0side.txt\0side-renamed.txt\x001\t0\tstaged-new.txt\0";

    // `git diff-files -z --raw --numstat`: modified, deleted (empty file), mode change.
    const RAW_UNSTAGED: &[u8] =
        b":100644 100644 96aabf3ef4cbee8f175966e4f2f23e8fe2240045 0000000000000000000000000000000000000000 M\0\
        a.txt\0:100644 000000 e69de29bb2d1d6434b8b29ae775ad8c2e48c5391 0000000000000000000000000000000000000000 D\0\
        empty.txt\0:100755 100644 4163036efa65bd4a469e752267498f01ea36a55c 0000000000000000000000000000000000000000 M\0\
        script.sh\0:100644 100644 91d552370eeb567aa1e93a74810ab2e7323295c6 0000000000000000000000000000000000000000 M\0\
        staged-new.txt\x001\t0\ta.txt\x000\t0\tempty.txt\x000\t0\tscript.sh\x001\t0\tstaged-new.txt\0";

    // `git diff-files -z -M --raw --numstat`: deleted file + intent-to-add file paired as a worktree rename.
    const RAW_UNSTAGED_ITA_RENAME: &[u8] =
        b":100644 100644 b3dfdd07edee4b9b7931e9fd7ff893894d299854 9af6b0952e3f62245cd513137a58c9bb91d9c449 R098\0\
        ita-src.txt\0ita-dst.txt\x001\t0\t\0ita-src.txt\0ita-dst.txt\0";

    // Submodule with a dirty worktree: same oid on both sides, numstat 0/0.
    const RAW_DIRTY_SUBMODULE: &[u8] =
        b":160000 160000 6616a9a889347a2147e684f6f4d365b33259109a 6616a9a889347a2147e684f6f4d365b33259109a M\0\
        libs/sub\x000\t0\tlibs/sub\0";

    // `git diff-index -z -M --raw --numstat <commit>` (no `--cached`): staged + unstaged edit, file deleted
    // on disk, intent-to-add file, staged rename with an unstaged edit, two touched (stat-only) files.
    const RAW_WORKTREE: &[u8] =
        b":100644 100644 de980441c3ab03a8c07dda1ad27b8a11f39deb1e 0000000000000000000000000000000000000000 M\0\
        a.txt\0:100644 000000 abaddc0b9edd523c69166a2c9f3a9e31a4c873e3 0000000000000000000000000000000000000000 D\0\
        del.txt\0:000000 100644 0000000000000000000000000000000000000000 ed286a73bf99c64552ec35258b7a043b9e905de6 A\0\
        ita.txt\0:100644 100644 2fa992c0b8b5c6acd2bdd4fa31de29d29799bdd5 0000000000000000000000000000000000000000 M\0\
        keep.txt\0:100644 100644 064061dc46b9e7b0b991f07f1fadb0843b970e3d 5ab7687c6fd764decf3d1077e30e66d9b970f85a R067\0\
        ren-src.txt\0ren-dst.txt\0:100644 100644 4151899b742fd6b1c873b177b9d13451682089bc 0000000000000000000000000000000000000000 M\0\
        touch.txt\x002\t1\ta.txt\x000\t1\tdel.txt\x001\t0\tita.txt\x001\t1\t\0ren-src.txt\0ren-dst.txt\0";

    // `touch`ed file: diff-files reports it but numstat finds no change.
    const RAW_STAT_DIRTY: &[u8] =
        b":100644 100644 45b983be36b73c0788dc9cbcb76cbb80fc7bb057 0000000000000000000000000000000000000000 M\0\
        f.txt\0";

    // diff-files during a merge with AA / DU / UD conflicts: duplicate paths, one stat-only entry.
    const RAW_CONFLICTS: &[u8] =
        b":000000 100644 0000000000000000000000000000000000000000 0000000000000000000000000000000000000000 U\0\
        aa.txt\0:100644 100644 b19a1e93bec1317dc6097229e12afaffbfa74dc2 0000000000000000000000000000000000000000 M\0\
        aa.txt\0:000000 100644 0000000000000000000000000000000000000000 0000000000000000000000000000000000000000 U\0\
        du.txt\0:000000 100644 0000000000000000000000000000000000000000 0000000000000000000000000000000000000000 U\0\
        ud.txt\0:100644 100644 b19a1e93bec1317dc6097229e12afaffbfa74dc2 0000000000000000000000000000000000000000 M\0\
        ud.txt\x000\t0\taa.txt\x004\t0\taa.txt\x000\t0\tdu.txt\x000\t0\tud.txt\0";

    // Names that look like options, numstat lines or raw headers; TAB and LF in names; rename of a TAB name.
    const RAW_SPECIAL: &[u8] =
        b":000000 100644 0000000000000000000000000000000000000000 39920f14ae68e9cea3c7e820cb6c10b06c319f8b A\0\
        --\0:100644 100644 0ff3bbb9c8bba2291654cd64067fa417ff54c508 d4de868f166881c4b312015004db3d4c27b0adce M\0\
        -dash.txt\0:100644 100644 4bcfe98e640c8284511312660fb8709b0afa888e 546a1f328336fa3f3475bf83b6ca9061ccd85ade M\0\
        1\t2\tfake.txt\0:100644 100644 f2ad6c76f0115a6ba5b00456a849810e7ec0af20 b706ef1b21ae8f4113ab16deeb43addf5d518d17 M\0\
        :colon.txt\0:100644 100644 587be6b4c3f93f93c489c0111bba5596147a26cb b77b4eb1d946f923f61785536da9ca5af6909f06 M\0\
        new\n\
        line.txt\0:100644 100644 0ff3bbb9c8bba2291654cd64067fa417ff54c508 0ff3bbb9c8bba2291654cd64067fa417ff54c508 R100\0\
        tab\tname.txt\0tab\trenamed.txt\x001\t0\t--\x001\t0\t-dash.txt\x001\t0\t1\t2\tfake.txt\0\
        1\t0\t:colon.txt\x001\t0\tnew\n\
        line.txt\x000\t0\t\0tab\tname.txt\0tab\trenamed.txt\0";

    // Blobless partial clone without lazy fetching: the `--raw` fallback (no numstat, no rename detection).
    const RAW_PARTIAL_CLONE: &[u8] =
        b":100644 000000 8178c76d627cade75005b40711b92f4177bc6cfc 0000000000000000000000000000000000000000 D\0\
        README.md\0:100644 100644 83536cff1521d812a0feee584bd568ea709389f8 8e498a275ff4d37b96e6a6496e6ef26af35109d4 M\0\
        bin.dat\0:000000 100644 0000000000000000000000000000000000000000 627165f5e27b6486daf3678a4edf4688de9aa47c A\0\
        copy-dst.txt\0:100644 100644 627165f5e27b6486daf3678a4edf4688de9aa47c 6774ffc32a1dfda3bb15bdc01cd082d3e6c79e3f M\0\
        copy-src.txt\0:100644 100644 75f18be4fd2a229531097e94374316da64baa0f2 4b63a83b9df0e310d96c3ce18f108358874aa0a8 M\0\
        crlf.txt\0:100644 100644 06cda4b7b93b86b74b1f1fc9b676a8f7936bc736 9880d0735333103ce1a0c913e1560cfd219f106f M\0\
        dashes.txt\0:000000 100644 0000000000000000000000000000000000000000 e69de29bb2d1d6434b8b29ae775ad8c2e48c5391 A\0\
        empty2.txt\0:120000 100644 8d14cbf983b3fad683171c9418998d9f68340823 3f899ea7ab51da801dbacbf633c168b0591d7765 T\0\
        link\0:100644 000000 35be908f97592ab48b79b15942eba5aa7959aab9 0000000000000000000000000000000000000000 D\0\
        long.txt\0:100644 100644 96bda30e0c2cef9a2f01fe00388a3c42180ba71a 5d9b138163b966315b60458d835cfbb3b3d77b63 M\0\
        nonl-new.txt\0:100644 100644 31cab939b11e973f3b3a88905fe7c5066c5dc46d 11f660feae9dd8551c364f0da9bac0483e94099c M\0\
        nonl-old.txt\0:100644 100644 daf3c426ac9136633240049e259e1e3f6d7dc26c f9f7b69167a2970e1eecc6f2bc11cb37e9c715c1 M\0\
        nonl.txt\0:000000 100644 0000000000000000000000000000000000000000 afdb0f7f895ef388c1c50e58a8ac4e73bc561458 A\0\
        renamed long.txt\0:100644 100755 4163036efa65bd4a469e752267498f01ea36a55c 4163036efa65bd4a469e752267498f01ea36a55c M\0\
        script.sh\0:100644 100644 7a44627979a55254e1f25e2bdfcfd8b269868610 d6bbe256e5c8d78a973aeff07d955365b7ffb5b4 M\0\
        \xed\x95\x9c\xea\xb8\x80 \xed\x8c\x8c\xec\x9d\xbc.txt\0";

    const LS_UNTRACKED: &[u8] =
        b"deep/er/f.txt\0nested/\0untracked-empty.txt\0untracked.bin\0\xec\x83\x88 \xed\x8c\x8c\xec\x9d\xbc.txt\0";

    #[test]
    fn details_root_commit() {
        let d = parse_commit_details(DETAILS_ROOT).unwrap();
        assert_eq!(d.oid, "0205c3f1b4884b20e95e8688230b654e76cd0067");
        assert!(d.parents.is_empty());
        assert_eq!(d.author.name, "Ann Author");
        assert_eq!(d.author.email, "ann@example.com");
        assert_eq!((d.author.time, d.author.tz.as_str()), (1700003600, "+0900"));
        assert_eq!(d.committer.name, "Cal Committer");
        assert_eq!((d.committer.time, d.committer.tz.as_str()), (1700003600, "-0130"));
        assert_eq!(d.subject, "root commit");
        assert_eq!(d.body, "Body line 1\nBody line 2\n\nSigned-off-by: Ann Author <ann@example.com>");
    }

    #[test]
    fn details_merge_commit() {
        let d = parse_commit_details(DETAILS_MERGE).unwrap();
        assert_eq!(d.parents, ["98f0816764393ec835690a6cf787b29b31234a73", "bfa833f9c85c35482d64acc7f227bd7ceb8e1d42"]);
        assert_eq!(d.subject, "Merge branch 'side'");
        assert_eq!(d.body, "");
        assert!(parse_commit_details(b"").is_none());
    }

    #[test]
    fn message_split() {
        assert_eq!(
            split_message("\n\n  Title \r\n\n\n    indented\r\n  code\n\n"),
            ("Title".into(), "    indented\n  code".into())
        );
        assert_eq!(split_message("only"), ("only".into(), String::new()));
        assert_eq!(split_message(""), (String::new(), String::new()));
        assert_eq!(split_message("a\n \n\t\nb\n"), ("a".into(), "b".into()));
    }

    #[test]
    fn raw_numstat_root_commit() {
        let files = parse_raw_numstat(RAW_ROOT);
        assert_eq!(files.len(), 15);
        assert!(files.iter().all(|f| f.status == "A" && f.old_mode.is_none() && f.old_path.is_none()));
        assert_eq!(counts(by_path(&files, "a.txt")), (Some(3), Some(0)));
        let bin = by_path(&files, "bin.dat");
        assert!(bin.binary);
        assert_eq!(counts(bin), (None, None));
        assert_eq!(counts(by_path(&files, "empty.txt")), (Some(0), Some(0)));
        assert_eq!(by_path(&files, "link").new_mode.as_deref(), Some("120000"));
        assert_eq!(counts(by_path(&files, "dir with space/file name.txt")), (Some(1), Some(0)));
        assert_eq!(counts(by_path(&files, "한글 파일.txt")), (Some(1), Some(0)));
        assert_eq!(totals(&files), (92, 0));
    }

    #[test]
    fn raw_numstat_edits() {
        let files = parse_raw_numstat(RAW_EDITS);
        assert_eq!(files.len(), 14);
        let readme = by_path(&files, "README.md");
        assert_eq!(
            (readme.status.as_str(), readme.old_mode.as_deref(), readme.new_mode.as_deref()),
            ("D", Some("100644"), None)
        );
        assert_eq!(counts(readme), (Some(0), Some(1)));
        let bin = by_path(&files, "bin.dat");
        assert!(bin.binary && bin.status == "M" && bin.additions.is_none());
        let renamed = by_path(&files, "renamed long.txt");
        assert_eq!(renamed.status, "R");
        assert_eq!(renamed.old_path.as_deref(), Some("long.txt"));
        assert_eq!(renamed.similarity, Some(96));
        assert_eq!(counts(renamed), (Some(1), Some(1)));
        let script = by_path(&files, "script.sh");
        assert_eq!((script.old_mode.as_deref(), script.new_mode.as_deref()), (Some("100644"), Some("100755")));
        assert_eq!(counts(script), (Some(0), Some(0)));
        let link = by_path(&files, "link");
        assert_eq!(
            (link.status.as_str(), link.old_mode.as_deref(), link.new_mode.as_deref()),
            ("T", Some("120000"), Some("100644"))
        );
        assert_eq!(counts(by_path(&files, "empty2.txt")), (Some(0), Some(0)));
        assert_eq!(counts(by_path(&files, "한글 파일.txt")), (Some(1), Some(1)));
        assert!(files.iter().all(|f| f.similarity.is_none() || f.status == "R"));
    }

    #[test]
    fn raw_numstat_copy() {
        let files = parse_raw_numstat(RAW_COPY);
        assert_eq!(files.len(), 14);
        let copy = by_path(&files, "copy-dst.txt");
        assert_eq!((copy.status.as_str(), copy.old_path.as_deref()), ("C", Some("copy-src.txt")));
        assert_eq!((copy.similarity, counts(copy)), (Some(100), (Some(0), Some(0))));
        let src = by_path(&files, "copy-src.txt");
        assert_eq!((src.status.as_str(), src.old_path.as_deref(), counts(src)), ("M", None, (Some(1), Some(0))));
        assert_eq!(counts(by_path(&files, "renamed long.txt")), (Some(1), Some(1)));
    }

    #[test]
    fn raw_numstat_submodules() {
        let files = parse_raw_numstat(RAW_SUBMODULE);
        assert_eq!(files.len(), 2);
        assert!(!files[0].submodule);
        assert!(files[1].submodule && files[1].status == "A");
        let bump = parse_raw_numstat(RAW_SUBMODULE_BUMP);
        assert!(bump[0].submodule);
        assert_eq!(counts(&bump[0]), (Some(1), Some(1)));
        let dirty = parse_raw_numstat(RAW_DIRTY_SUBMODULE);
        assert_eq!(dirty.len(), 1);
        assert!(dirty[0].submodule);
        assert_eq!(counts(&dirty[0]), (Some(0), Some(0)));
    }

    #[test]
    fn raw_numstat_merge_and_stash() {
        let files = parse_raw_numstat(RAW_MERGE);
        assert_eq!(files.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(), ["a.txt", "side.txt"]);
        assert_eq!(counts(&files[0]), (Some(1), Some(1)));
        let files = parse_raw_numstat(RAW_STASH_UNTRACKED_WITH_ID);
        assert_eq!(files.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(), ["ud/inner.txt", "untracked new.txt"]);
        assert!(files.iter().all(|f| counts(f) == (Some(1), Some(0))));
    }

    #[test]
    fn raw_numstat_worktree() {
        let staged = parse_raw_numstat(RAW_STAGED);
        assert_eq!(staged.len(), 3);
        let r = by_path(&staged, "side-renamed.txt");
        assert_eq!((r.status.as_str(), r.old_path.as_deref(), r.similarity), ("R", Some("side.txt"), Some(100)));
        assert_eq!(by_path(&staged, "staged-new.txt").status, "A");
        let unstaged = parse_raw_numstat(RAW_UNSTAGED);
        assert_eq!(unstaged.len(), 4);
        assert_eq!(by_path(&unstaged, "empty.txt").status, "D");
        assert_eq!(by_path(&unstaged, "script.sh").new_mode.as_deref(), Some("100644"));
        assert_eq!(counts(by_path(&unstaged, "a.txt")), (Some(1), Some(0)));
        let ita = without_unmerged(without_stat_only(parse_raw_numstat(RAW_UNSTAGED_ITA_RENAME)));
        assert_eq!(ita.len(), 1);
        assert_eq!(
            (ita[0].status.as_str(), ita[0].path.as_str(), ita[0].old_path.as_deref(), ita[0].similarity),
            ("R", "ita-dst.txt", Some("ita-src.txt"), Some(98))
        );
        assert_eq!(counts(&ita[0]), (Some(1), Some(0)));
    }

    #[test]
    fn raw_numstat_commit_vs_worktree() {
        let files = without_stat_only(parse_raw_numstat(RAW_WORKTREE));
        let summary: Vec<_> =
            files.iter().map(|f| (f.status.as_str(), f.path.as_str(), f.old_path.as_deref(), counts(f))).collect();
        assert_eq!(
            summary,
            [
                ("M", "a.txt", None, (Some(2), Some(1))),
                ("D", "del.txt", None, (Some(0), Some(1))),
                ("A", "ita.txt", None, (Some(1), Some(0))),
                ("R", "ren-dst.txt", Some("ren-src.txt"), (Some(1), Some(1))),
            ]
        );
        assert_eq!(files[3].similarity, Some(67));
    }

    #[test]
    fn stat_only_and_unmerged_entries() {
        let files = parse_raw_numstat(RAW_STAT_DIRTY);
        assert_eq!((files.len(), counts(&files[0]), files[0].binary), (1, (None, None), false));
        assert!(without_stat_only(files).is_empty());
        let files = parse_raw_numstat(RAW_CONFLICTS);
        assert_eq!(files.len(), 5);
        assert_eq!(
            (files[4].status.as_str(), files[4].path.as_str(), counts(&files[4])),
            ("M", "ud.txt", (None, None))
        );
        let files = without_stat_only(files);
        let summary: Vec<_> = files.iter().map(|f| (f.status.as_str(), f.path.as_str(), counts(f))).collect();
        assert_eq!(
            summary,
            [
                ("U", "aa.txt", (Some(0), Some(0))),
                ("M", "aa.txt", (Some(4), Some(0))),
                ("U", "du.txt", (Some(0), Some(0))),
                ("U", "ud.txt", (Some(0), Some(0))),
            ]
        );
        assert!(without_unmerged(files).is_empty());
    }

    #[test]
    fn raw_numstat_special_names() {
        let files = parse_raw_numstat(RAW_SPECIAL);
        let paths: Vec<_> = files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, ["--", "-dash.txt", "1\t2\tfake.txt", ":colon.txt", "new\nline.txt", "tab\trenamed.txt"]);
        assert!(files[..5].iter().all(|f| counts(f) == (Some(1), Some(0))));
        assert_eq!(files[5].old_path.as_deref(), Some("tab\tname.txt"));
        assert_eq!(counts(&files[5]), (Some(0), Some(0)));
    }

    #[test]
    fn raw_numstat_pairs_by_path_when_out_of_order() {
        let buf =
            b":100644 100644 aaaa bbbb M\0a.txt\0:100644 100644 cccc dddd M\0b.txt\x002\t3\tb.txt\x004\t5\ta.txt\0";
        let files = parse_raw_numstat(buf);
        assert_eq!(counts(by_path(&files, "a.txt")), (Some(4), Some(5)));
        assert_eq!(counts(by_path(&files, "b.txt")), (Some(2), Some(3)));
        assert!(parse_raw_numstat(b"").is_empty());
        assert!(parse_raw_numstat(b":100644 100644 aaaa bbbb R100\0only-old\0").is_empty());
    }

    #[test]
    fn raw_without_numstat() {
        let files = parse_raw_numstat(RAW_PARTIAL_CLONE);
        assert_eq!(files.len(), 15);
        assert!(files.iter().all(|f| counts(f) == (None, None) && !f.binary && f.old_path.is_none()));
        assert_eq!(by_path(&files, "long.txt").status, "D");
        assert_eq!(by_path(&files, "renamed long.txt").status, "A");
        assert_eq!(by_path(&files, "script.sh").new_mode.as_deref(), Some("100755"));
        assert_eq!(totals(&files), (0, 0));
    }

    #[test]
    fn untracked_list() {
        let files = parse_untracked(LS_UNTRACKED);
        let paths: Vec<_> = files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, ["deep/er/f.txt", "nested/", "untracked-empty.txt", "untracked.bin", "새 파일.txt"]);
        assert!(files.iter().all(|f| f.status == "?" && f.additions.is_none() && !f.binary));
    }

    #[test]
    fn input_validation() {
        assert!(validate_oid("abcd").is_ok());
        assert!(validate_oid(&"F".repeat(64)).is_ok());
        assert!(validate_oid("abc").is_err());
        assert!(validate_oid(&"a".repeat(65)).is_err());
        assert!(validate_oid("abcg").is_err());
        assert!(validate_rev("HEAD~2").is_ok());
        assert!(validate_rev("abcd^1").is_ok());
        assert!(validate_rev("--output=x").is_err());
        assert!(validate_rev("").is_err());
        assert!(validate_path("-dash.txt").is_ok());
        assert!(validate_path("").is_err());
        assert!(validate_path("a\0b").is_err());
        assert!(relative_path("dir/a.txt").is_ok());
        assert!(relative_path("nested/").is_ok());
        assert!(relative_path("../x").is_err());
        assert!(relative_path("a/../../x").is_err());
        assert!(relative_path("/etc/passwd").is_err());
        assert!(worktree_dir("relative/dir").is_err());
    }
}
