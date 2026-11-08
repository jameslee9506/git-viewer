//! `git blame --porcelain`.

use crate::error::{AppError, AppResult};
use crate::git::cmd::{lossy, unquote_c, Git};
use crate::model::{BlameCommit, BlameData, BlameHunk};
use std::collections::HashMap;
use std::io::Read;
use std::path::{Component, Path};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// Bytes inspected by git's binary heuristic (a NUL among the first 8000 bytes).
const BINARY_PROBE: usize = 8000;

/// Blame `path` at `rev` (None = working tree file in `cwd`, defaulting to `repo`). `ignore_whitespace`
/// passes `-w`: lines whose only change was whitespace keep their earlier commit. Once `cancel` is set
/// (a newer blame superseded this one) git is killed, or not started, and the error is "cancelled".
pub fn blame(
    repo: &Path,
    path: &str,
    rev: Option<&str>,
    cwd: Option<&str>,
    ignore_whitespace: bool,
    cancel: Option<Arc<AtomicBool>>,
) -> AppResult<BlameData> {
    if !is_repo_path(path) {
        return Err(AppError::msg(format!("invalid path: {path}")));
    }
    if let Some(r) = rev {
        if r.is_empty() || r.starts_with('-') || r.contains('\0') {
            return Err(AppError::msg(format!("invalid revision: {r}")));
        }
    }
    let dir = cwd.map(Path::new).unwrap_or(repo);
    if !dir.is_dir() {
        return Err(AppError::msg(format!("not a directory: {}", dir.display())));
    }
    // Superseded while queued (behind other work or busy process slots): skip the git calls.
    let check_cancel = || match &cancel {
        Some(c) if c.load(Ordering::Relaxed) => Err(AppError::msg("cancelled")),
        _ => Ok(()),
    };
    check_cancel()?;
    if is_binary(dir, path, rev) {
        return Err(AppError::msg(format!("binary file: {path}")));
    }
    let run = |no_global_config: bool| {
        check_cancel()?;
        let mut git = Git::new(dir).args(["blame", "--porcelain"]);
        if ignore_whitespace {
            git = git.arg("-w");
        }
        if no_global_config {
            git = git.env("GIT_CONFIG_GLOBAL", "/dev/null");
        }
        if let Some(r) = rev {
            git = git.arg(r);
        }
        if let Some(c) = &cancel {
            git = git.cancel_on(c.clone());
        }
        git.arg("--").arg(path).literal_paths().timeout(Duration::from_secs(60)).run()
    };
    let out = match run(false) {
        Ok(out) => out,
        // A global `blame.ignoreRevsFile = .git-blame-ignore-revs` aborts blame in every repo that lacks
        // the file, and neither `-c` nor `--ignore-revs-file=` can clear it: retry without global config.
        Err(e) if e.to_string().contains("could not open object name list") => run(true)?,
        Err(e) => return Err(e),
    };
    Ok(parse_porcelain(&out, path))
}

/// Parse `git blame --porcelain` output of `path`: one entry per line, consecutive lines of the
/// same commit with consecutive original line numbers (and the same original path) form a hunk.
pub fn parse_porcelain(buf: &[u8], path: &str) -> BlameData {
    let mut data = BlameData { path: path.to_string(), ..Default::default() };
    // Latest `filename` per commit. Git prints it with the commit info only, or for every group
    // of a commit that is blamed under several paths.
    let mut filenames: HashMap<&str, String> = HashMap::new();
    // Header of the line whose key lines are being read: (oid, line number in that commit).
    let mut pending: Option<(&str, u32)> = None;
    for line in buf.split(|b| *b == b'\n') {
        if let Some(text) = line.strip_prefix(b"\t") {
            if let Some((oid, orig_line)) = pending.take() {
                let file = filenames.get(oid).map_or(path, String::as_str);
                push_line(&mut data, oid, orig_line, file, lossy(text));
            }
            continue;
        }
        let Some((oid, _)) = pending else {
            pending = parse_header(line);
            if let Some((oid, _)) = pending {
                if !data.commits.contains_key(oid) {
                    data.commits.insert(oid.to_string(), new_commit(oid));
                }
            }
            continue;
        };
        let (key, value) = match line.iter().position(|b| *b == b' ') {
            Some(i) => (&line[..i], &line[i + 1..]),
            None => (line, &[][..]),
        };
        if key == b"filename" {
            filenames.insert(oid, lossy(&unquote_c(value)));
            continue;
        }
        let Some(c) = data.commits.get_mut(oid) else { continue };
        match key {
            b"author" => c.author = lossy(value),
            b"author-mail" => {
                let v = value.strip_prefix(b"<").unwrap_or(value);
                c.email = lossy(v.strip_suffix(b">").unwrap_or(v));
            }
            b"author-time" => c.time = std::str::from_utf8(value).ok().and_then(|s| s.parse().ok()).unwrap_or(0),
            b"summary" => c.summary = lossy(value),
            _ => {}
        }
    }
    data
}

/// Append one line, extending the last hunk when it continues it.
fn push_line(data: &mut BlameData, oid: &str, orig_line: u32, file: &str, text: String) {
    let start = data.lines.len() as u32;
    data.lines.push(text);
    if let Some(h) = data.hunks.last_mut() {
        let same_file = h.orig_path.as_deref().unwrap_or(data.path.as_str()) == file;
        if h.oid == oid && h.orig_line + h.count == orig_line && same_file {
            h.count += 1;
            return;
        }
    }
    data.hunks.push(BlameHunk {
        start,
        count: 1,
        oid: oid.to_string(),
        orig_path: (file != data.path).then(|| file.to_string()),
        orig_line,
    });
}

/// `<oid> <orig line> <final line>[ <count>]` → (oid, orig line). Final line numbers are implied
/// by the output order (porcelain output is sorted).
fn parse_header(line: &[u8]) -> Option<(&str, u32)> {
    let mut it = line.split(|b| *b == b' ');
    let oid = it.next()?;
    if oid.len() < 40 || !oid.iter().all(u8::is_ascii_hexdigit) {
        return None;
    }
    let orig = line_number(it.next()?)?;
    line_number(it.next()?)?;
    Some((std::str::from_utf8(oid).ok()?, orig))
}

fn line_number(b: &[u8]) -> Option<u32> {
    std::str::from_utf8(b).ok()?.parse().ok()
}

fn new_commit(oid: &str) -> BlameCommit {
    BlameCommit {
        oid: oid.to_string(),
        author: String::new(),
        email: String::new(),
        time: 0,
        summary: String::new(),
        uncommitted: oid.bytes().all(|b| b == b'0'),
    }
}

/// Repo-relative path: not empty, not absolute, no `..` components. A leading `-` is fine (files can be
/// named like that): the path only ever follows `--` or sits inside a `<rev>:<path>` object name.
fn is_repo_path(path: &str) -> bool {
    !path.is_empty()
        && !path.contains('\0')
        && Path::new(path).components().all(|c| matches!(c, Component::Normal(_) | Component::CurDir))
}

/// Git's binary heuristic on the content to blame. Content that cannot be read counts as text so
/// that blame itself reports the problem (missing path, bad revision, directory, ...).
fn is_binary(dir: &Path, path: &str, rev: Option<&str>) -> bool {
    let head = match rev {
        Some(r) => {
            // Only the first bytes count: git is stopped once they are in, so a huge blob is never read whole.
            let mut head = Vec::with_capacity(BINARY_PROBE);
            let out = Git::new(dir).args(["cat-file", "blob"]).arg(format!("{r}:{path}")).output_streaming(|chunk| {
                head.extend_from_slice(&chunk[..chunk.len().min(BINARY_PROBE - head.len())]);
                head.len() < BINARY_PROBE
            });
            match out {
                Ok(o) if o.code == 0 || head.len() == BINARY_PROBE => head,
                _ => return false,
            }
        }
        None => {
            let full = dir.join(path);
            // Symlinks are blamed by their target path text, which is never binary.
            if !std::fs::symlink_metadata(&full).is_ok_and(|m| m.is_file()) {
                return false;
            }
            let mut head = Vec::with_capacity(BINARY_PROBE);
            let read = std::fs::File::open(&full).and_then(|f| f.take(BINARY_PROBE as u64).read_to_end(&mut head));
            if read.is_err() {
                return false;
            }
            head
        }
    };
    head[..head.len().min(BINARY_PROBE)].contains(&0)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Real `git blame --porcelain` output (git 2.48.1).

    const ROOT: &str = "3ecfa992180980c1effdacfc91129459f1184cf4";
    const BOB: &str = "1a41f5e3f2383bcc64087a1e0344e7024dc6a4ea";
    const ZERO: &str = "0000000000000000000000000000000000000000";

    /// Working tree blame of `new.txt` (renamed from `old.txt`) with two uncommitted lines; the last
    /// line has no trailing newline in the file.
    const WORKTREE: &[u8] = b"3ecfa992180980c1effdacfc91129459f1184cf4 1 1 1\n\
    author Ann Author\n\
    author-mail <ann@example.com>\n\
    author-time 1700003600\n\
    author-tz +0900\n\
    committer Cal Committer\n\
    committer-mail <cal@example.com>\n\
    committer-time 1700003600\n\
    committer-tz +0100\n\
    summary root: add files\n\
    boundary\n\
    filename old.txt\n\
    \tL1\n\
    0000000000000000000000000000000000000000 2 2 1\n\
    author Not Committed Yet\n\
    author-mail <not.committed.yet>\n\
    author-time 1794106002\n\
    author-tz +0900\n\
    committer Not Committed Yet\n\
    committer-mail <not.committed.yet>\n\
    committer-time 1794106002\n\
    committer-tz +0900\n\
    summary Version of new.txt from new.txt\n\
    previous fdeb267f0fc381cfae8b962fbab638ebc491423d new.txt\n\
    filename new.txt\n\
    \tL2 wip\n\
    ead182a7af70fa23069026a4ed0ca65ac676dd3f 3 3 1\n\
    author Ann Author\n\
    author-mail <ann@example.com>\n\
    author-time 1700007200\n\
    author-tz +0900\n\
    committer Cal Committer\n\
    committer-mail <cal@example.com>\n\
    committer-time 1700007200\n\
    committer-tz +0100\n\
    summary change L3, drop b\n\
    previous 3ecfa992180980c1effdacfc91129459f1184cf4 old.txt\n\
    filename old.txt\n\
    \tL3 changed\n\
    3ecfa992180980c1effdacfc91129459f1184cf4 4 4 1\n\
    \tL4\n\
    1a41f5e3f2383bcc64087a1e0344e7024dc6a4ea 5 5 2\n\
    author Bob Second\n\
    author-mail <bob@example.com>\n\
    author-time 1700014400\n\
    author-tz +0900\n\
    committer Cal Committer\n\
    committer-mail <cal@example.com>\n\
    committer-time 1700014400\n\
    committer-tz +0100\n\
    summary bob edits\n\
    previous 1b62299e7a630b2b0ed67c79a75ea25f28fafd7c new.txt\n\
    filename new.txt\n\
    \tL5 by bob\n\
    1a41f5e3f2383bcc64087a1e0344e7024dc6a4ea 6 6\n\
    \tL6 new\n\
    0000000000000000000000000000000000000000 7 7 1\n\
    \tL7 wip\n";

    /// `plain.txt` at HEAD, renamed from a name with a TAB (C-quoted `filename`).
    const RENAMED_QUOTED: &[u8] = b"3ecfa992180980c1effdacfc91129459f1184cf4 1 1 2\n\
    author Ann Author\n\
    author-mail <ann@example.com>\n\
    author-time 1700003600\n\
    author-tz +0900\n\
    committer Cal Committer\n\
    committer-mail <cal@example.com>\n\
    committer-time 1700003600\n\
    committer-tz +0100\n\
    summary root: add files\n\
    boundary\n\
    filename \"tab\\tname.txt\"\n\
    \tq1\n\
    3ecfa992180980c1effdacfc91129459f1184cf4 2 2\n\
    \tq2\n\
    1a41f5e3f2383bcc64087a1e0344e7024dc6a4ea 3 3 1\n\
    author Bob Second\n\
    author-mail <bob@example.com>\n\
    author-time 1700014400\n\
    author-tz +0900\n\
    committer Cal Committer\n\
    committer-mail <cal@example.com>\n\
    committer-time 1700014400\n\
    committer-tz +0100\n\
    summary bob edits\n\
    previous 1b62299e7a630b2b0ed67c79a75ea25f28fafd7c plain.txt\n\
    filename plain.txt\n\
    \tq3\n";

    /// CRLF lines and a last line without newline.
    const CRLF: &[u8] = b"3ecfa992180980c1effdacfc91129459f1184cf4 1 1 3\n\
    author Ann Author\n\
    author-mail <ann@example.com>\n\
    author-time 1700003600\n\
    author-tz +0900\n\
    committer Cal Committer\n\
    committer-mail <cal@example.com>\n\
    committer-time 1700003600\n\
    committer-tz +0100\n\
    summary root: add files\n\
    boundary\n\
    filename crlf.txt\n\
    \tcrlf1\r\n\
    3ecfa992180980c1effdacfc91129459f1184cf4 2 2\n\
    \tcrlf2\r\n\
    3ecfa992180980c1effdacfc91129459f1184cf4 3 3\n\
    \tno-eol\n";

    /// One commit, two groups: original line 2 was deleted later, so lines 1 and 3-4 don't join.
    const GAP: &[u8] = b"3ecfa992180980c1effdacfc91129459f1184cf4 1 1 1\n\
    author Ann Author\n\
    author-mail <ann@example.com>\n\
    author-time 1700003600\n\
    author-tz +0900\n\
    committer Cal Committer\n\
    committer-mail <cal@example.com>\n\
    committer-time 1700003600\n\
    committer-tz +0100\n\
    summary root: add files\n\
    boundary\n\
    filename del.txt\n\
    \ta\n\
    3ecfa992180980c1effdacfc91129459f1184cf4 3 2 2\n\
    \tc\n\
    3ecfa992180980c1effdacfc91129459f1184cf4 4 3\n\
    \td\n";

    fn hunks(d: &BlameData) -> Vec<(u32, u32, &str, Option<&str>, u32)> {
        d.hunks.iter().map(|h| (h.start, h.count, h.oid.as_str(), h.orig_path.as_deref(), h.orig_line)).collect()
    }

    #[test]
    fn worktree_blame_with_uncommitted_lines_and_rename() {
        let d = parse_porcelain(WORKTREE, "new.txt");
        assert_eq!(d.path, "new.txt");
        assert_eq!(d.lines, vec!["L1", "L2 wip", "L3 changed", "L4", "L5 by bob", "L6 new", "L7 wip"]);
        assert_eq!(
            hunks(&d),
            vec![
                (0, 1, ROOT, Some("old.txt"), 1),
                (1, 1, ZERO, None, 2),
                (2, 1, "ead182a7af70fa23069026a4ed0ca65ac676dd3f", Some("old.txt"), 3),
                (3, 1, ROOT, Some("old.txt"), 4),
                (4, 2, BOB, None, 5),
                (6, 1, ZERO, None, 7),
            ]
        );
        assert_eq!(d.commits.len(), 4);
        let root = &d.commits[ROOT];
        assert_eq!(
            (root.oid.as_str(), root.author.as_str(), root.email.as_str(), root.time, root.summary.as_str()),
            (ROOT, "Ann Author", "ann@example.com", 1700003600, "root: add files")
        );
        assert!(!root.uncommitted);
        let zero = &d.commits[ZERO];
        assert!(zero.uncommitted);
        assert_eq!((zero.author.as_str(), zero.email.as_str()), ("Not Committed Yet", "not.committed.yet"));
        assert_eq!(d.commits[BOB].author, "Bob Second");
        assert_eq!(d.commits[BOB].summary, "bob edits");
    }

    #[test]
    fn hunks_cover_every_line_once() {
        for (buf, path) in [(WORKTREE, "new.txt"), (RENAMED_QUOTED, "plain.txt"), (CRLF, "crlf.txt"), (GAP, "del.txt")] {
            let d = parse_porcelain(buf, path);
            let mut next = 0;
            for h in &d.hunks {
                assert_eq!(h.start, next);
                assert!(h.count > 0);
                assert!(d.commits.contains_key(&h.oid));
                next += h.count;
            }
            assert_eq!(next as usize, d.lines.len());
        }
    }

    #[test]
    fn quoted_filename_from_rename() {
        let d = parse_porcelain(RENAMED_QUOTED, "plain.txt");
        assert_eq!(d.lines, vec!["q1", "q2", "q3"]);
        assert_eq!(hunks(&d), vec![(0, 2, ROOT, Some("tab\tname.txt"), 1), (2, 1, BOB, None, 3)]);
    }

    #[test]
    fn keeps_carriage_returns() {
        let d = parse_porcelain(CRLF, "crlf.txt");
        assert_eq!(d.lines, vec!["crlf1\r", "crlf2\r", "no-eol"]);
        assert_eq!(hunks(&d), vec![(0, 3, ROOT, None, 1)]);
    }

    #[test]
    fn non_consecutive_original_lines_split_hunks() {
        let d = parse_porcelain(GAP, "del.txt");
        assert_eq!(d.lines, vec!["a", "c", "d"]);
        assert_eq!(hunks(&d), vec![(0, 1, ROOT, None, 1), (1, 2, ROOT, None, 3)]);
        assert_eq!(d.commits.len(), 1);
    }

    #[test]
    fn adjacent_groups_merge_and_per_group_filenames() {
        let a = "a".repeat(40);
        let b = "b".repeat(40);
        let buf = format!(
            "{a} 1 1 1\nauthor A\nauthor-mail <>\nauthor-time 5\nsummary s\nunknown-key x\nfilename f.txt\n\tone\n\
             {a} 2 2 1\n\ttwo\n\
             {b} 7 3 1\nauthor B\nauthor-mail <b@x>\nauthor-time 6\nsummary t\nfilename old.txt\n\tthree\n\
             {b} 8 4 1\nfilename f.txt\n\tfour\n"
        );
        let d = parse_porcelain(buf.as_bytes(), "f.txt");
        assert_eq!(d.lines, vec!["one", "two", "three", "four"]);
        assert_eq!(hunks(&d), vec![(0, 2, a.as_str(), None, 1), (2, 1, b.as_str(), Some("old.txt"), 7), (3, 1, b.as_str(), None, 8)]);
        assert_eq!(d.commits[&a].email, "");
        assert_eq!((d.commits[&b].time, d.commits[&b].email.as_str()), (6, "b@x"));
    }

    #[test]
    fn empty_file() {
        let d = parse_porcelain(b"", "empty.txt");
        assert_eq!(d.path, "empty.txt");
        assert!(d.lines.is_empty() && d.hunks.is_empty() && d.commits.is_empty());
    }

    #[test]
    fn cancelled_blame_runs_no_git() {
        let dir = std::env::temp_dir();
        let cancelled = Some(Arc::new(AtomicBool::new(true)));
        let err = blame(&dir, "a.txt", Some("HEAD"), None, false, cancelled).unwrap_err();
        assert_eq!(err.to_string(), "cancelled");
        // Argument errors still come first.
        let err = blame(&dir, "../a.txt", None, None, false, Some(Arc::new(AtomicBool::new(true)))).unwrap_err();
        assert_eq!(err.to_string(), "invalid path: ../a.txt");
    }

    #[test]
    fn repo_path_validation() {
        assert!(is_repo_path("a.txt") && is_repo_path("dir with space/x") && is_repo_path("./a"));
        assert!(is_repo_path("-dash.txt") && is_repo_path("dir/-x"));
        assert!(!is_repo_path("") && !is_repo_path("/etc/passwd") && !is_repo_path("a\0b"));
        assert!(!is_repo_path("../x") && !is_repo_path("a/../../x"));
    }
}
