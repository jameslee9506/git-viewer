//! Per-file history (`git log --follow`).

use crate::error::{AppError, AppResult};
use crate::git::cmd::{lossy, Git};
use crate::model::HistoryEntry;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Record separator emitted before every commit.
const RS: u8 = 0x1e;
/// Six NUL-terminated fields per commit, then (when the commit touches the path) `\n` and the
/// `--name-status -z` entries.
const FORMAT: &str = "--format=%x1e%H%x00%P%x00%aN%x00%aE%x00%at%x00%s";

/// Commits touching `path` starting at `rev` (None = HEAD). `limit` 0 = unlimited. A newer call for
/// the same `repo` supersedes this one (see `RUNNING`): its git is killed, or never started, and the
/// call fails with "cancelled".
pub fn file_history(repo: &Path, path: &str, rev: Option<&str>, follow: bool, limit: u32) -> AppResult<Vec<HistoryEntry>> {
    // The path always follows `--`, so a leading `-` is a plain file name.
    if path.is_empty() || path.contains('\0') {
        return Err(AppError::msg(format!("invalid path: {path}")));
    }
    let start = rev.unwrap_or("HEAD");
    if start.is_empty() || start.starts_with('-') || start.contains('\0') {
        return Err(AppError::msg(format!("invalid revision: {start}")));
    }
    if !repo.is_dir() {
        return Err(AppError::msg(format!("not a directory: {}", repo.display())));
    }
    let run = Run::begin(repo);
    // log.showRoot=false would drop the root commit's entries; --no-follow overrides log.follow=true;
    // merges carry no entries, so excluding them keeps --max-count exact.
    let mut git = Git::new(repo)
        .args(["-c", "log.showRoot=true", "log", "-z", "-M", "--name-status", "--no-merges", FORMAT])
        .arg(if follow { "--follow" } else { "--no-follow" })
        .cancel_on(run.cancel.clone());
    if limit > 0 {
        git = git.arg(format!("--max-count={limit}"));
    }
    // A file that changed rarely makes git walk the whole history; bound it like blame so abandoned
    // requests cannot hold git process slots indefinitely.
    match git.arg(start).arg("--").arg(path).literal_paths().timeout(Duration::from_secs(60)).run() {
        Ok(out) => Ok(parse_log(&out, path)),
        // Superseded: nothing will show this history, so skip the unborn-HEAD probe too.
        Err(e) if run.cancelled() => Err(e),
        Err(_) if start == "HEAD" && head_unborn(repo) => Ok(Vec::new()),
        Err(e) => Err(e),
    }
}

/// Cancel flag of the running history per repository (at most one entry each). A repo tab shows one
/// file history at a time and drops the result of every request but its newest, so a new request
/// (another file, or HEAD moved) kills the run it supersedes instead of leaving a slow `--follow` walk
/// to hold a git process slot until its timeout.
static RUNNING: Mutex<Vec<(PathBuf, Arc<AtomicBool>)>> = Mutex::new(Vec::new());

/// A registered history run; unregisters on drop.
struct Run {
    cancel: Arc<AtomicBool>,
}

impl Run {
    /// Register a run in `repo`, cancelling the one still going there.
    fn begin(repo: &Path) -> Run {
        let cancel = Arc::new(AtomicBool::new(false));
        let mut running = RUNNING.lock().unwrap_or_else(|e| e.into_inner());
        match running.iter_mut().find(|(r, _)| r.as_path() == repo) {
            Some((_, flag)) => std::mem::replace(flag, cancel.clone()).store(true, Ordering::Relaxed),
            None => running.push((repo.to_path_buf(), cancel.clone())),
        }
        Run { cancel }
    }

    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
}

impl Drop for Run {
    fn drop(&mut self) {
        RUNNING.lock().unwrap_or_else(|e| e.into_inner()).retain(|(_, flag)| !Arc::ptr_eq(flag, &self.cancel));
    }
}

/// Cancel the history running in `repo`, if any (its tab closed: nothing will show the result, and the
/// walk would hold a git process slot until it ends or times out).
pub fn cancel(repo: &Path) {
    RUNNING.lock().unwrap_or_else(|e| e.into_inner()).retain(|(r, flag)| {
        let mine = r.as_path() == repo;
        if mine {
            flag.store(true, Ordering::Relaxed);
        }
        !mine
    });
}

/// HEAD names a branch without commits yet.
fn head_unborn(repo: &Path) -> bool {
    Git::new(repo).args(["rev-parse", "-q", "--verify", "HEAD"]).output().is_ok_and(|o| o.code == 1)
}

/// One `--name-status` entry; `old` is set for renames and copies.
struct Change<'a> {
    status: u8,
    old: Option<&'a [u8]>,
    path: &'a [u8],
}

/// Parse `git log -z --name-status` output in the FORMAT layout. Commits without entries (merges)
/// are skipped. When a commit lists several entries, the one for the followed name wins; a rename
/// or copy makes its source the followed name for older commits (as `--follow` does).
pub fn parse_log(buf: &[u8], path: &str) -> Vec<HistoryEntry> {
    let mut out = Vec::new();
    let mut followed: Vec<u8> = path.as_bytes().to_vec();
    let mut changes: Vec<Change> = Vec::new();
    let mut pos = 0;
    while let Some(i) = memchr::memchr(RS, &buf[pos..]) {
        pos += i + 1;
        let empty: &[u8] = &[];
        let mut f = [empty; 6];
        for slot in &mut f {
            match field(buf, &mut pos) {
                Some(v) => *slot = v,
                None => return out,
            }
        }
        changes.clear();
        if buf.get(pos) == Some(&b'\n') {
            pos += 1;
            // Status tokens never start with RS, so an RS here starts the next record (paths may contain one).
            while pos < buf.len() && buf[pos] != RS {
                let Some(change) = next_change(buf, &mut pos) else { break };
                changes.push(change);
            }
        }
        let Some(c) = changes.iter().find(|c| c.path == followed.as_slice()).or(changes.first()) else { continue };
        if let Some(old) = c.old {
            followed = old.to_vec();
        }
        out.push(HistoryEntry {
            oid: lossy(f[0]),
            parents: f[1].split(|b| *b == b' ').filter(|p| !p.is_empty()).map(lossy).collect(),
            author: lossy(f[2]),
            email: lossy(f[3]),
            time: std::str::from_utf8(f[4]).ok().and_then(|s| s.parse().ok()).unwrap_or(0),
            subject: lossy(f[5]),
            path: lossy(c.path),
            old_path: c.old.map(lossy),
            status: char::from(c.status).to_string(),
        });
    }
    out
}

/// `<status>\0<path>\0`, or `R<score>\0<old>\0<new>\0` / `C<score>\0<old>\0<new>\0`.
fn next_change<'a>(buf: &'a [u8], pos: &mut usize) -> Option<Change<'a>> {
    let status = *field(buf, pos)?.first()?;
    let first = field(buf, pos)?;
    if matches!(status, b'R' | b'C') {
        let second = field(buf, pos)?;
        Some(Change { status, old: Some(first), path: second })
    } else {
        Some(Change { status, old: None, path: first })
    }
}

/// Next NUL-terminated field at `pos`; None when the terminator is missing (truncated output).
fn field<'a>(buf: &'a [u8], pos: &mut usize) -> Option<&'a [u8]> {
    let rest = buf.get(*pos..)?;
    let end = memchr::memchr(0, rest)?;
    *pos += end + 1;
    Some(&rest[..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    // Real `git log -z -M --name-status --format=%x1e%H%x00%P%x00%aN%x00%aE%x00%at%x00%s` output
    // (git 2.48.1).

    /// `--follow` of `b 한.txt`: mode change, edit, rename with edit (R068), then the history of
    /// `a.txt` through two merged branches back to the root commit.
    const FOLLOW: &[u8] = b"\x1ec84d9efd1c8b340b7fdb9e4eb8036e5ef7a84099\0e5bc7a2801fb6586d26817f6a495e43e6612ce4d\0Ann Author\0ann@example.com\x001700061200\0make b executable\0\nM\0b \xed\x95\x9c.txt\0\
    \x1ee5bc7a2801fb6586d26817f6a495e43e6612ce4d\x003616b768b52d451f451e55b205858df118bbb9e6\0Ann Author\0ann@example.com\x001700057600\0edit b\0\nM\0b \xed\x95\x9c.txt\0\
    \x1e3616b768b52d451f451e55b205858df118bbb9e6\x001d5bb77a9a86f2eee6a126e30f95e66e93d4aaaf\0Ann Author\0ann@example.com\x001700054000\0rename a -> b with edit\0\nR068\0a.txt\0b \xed\x95\x9c.txt\0\
    \x1e787bdad61eae93de67da95463aa1ce33451de0f9\x009865c96114e3a56b2e9f709bca3774a613857889\0Ann Author\0ann@example.com\x001700043200\0main side edit\0\nM\0a.txt\0\
    \x1e4a9de4fd3a251fc95a29d57ebd7985bc12427ff9\x009865c96114e3a56b2e9f709bca3774a613857889\0Ann Author\0ann@example.com\x001700039600\0evil side edit\0\nM\0a.txt\0\
    \x1e2760f540e35e521675c80c8c4eda70787d7fef37\x0054c1e9bb3f5149c2de7e7e7f97709dcfc162ec23\0Ann Author\0ann@example.com\x001700028800\0feat: edit a line 10\0\nM\0a.txt\0\
    \x1e54c1e9bb3f5149c2de7e7e7f97709dcfc162ec23\x0000e8e9bae805d3a52163cef76426f280e6cc7511\0Ann Author\0ann@example.com\x001700025200\0edit a line 1\0\nM\0a.txt\0\
    \x1e00e8e9bae805d3a52163cef76426f280e6cc7511\0\0Ann Author\0ann@example.com\x001700021600\0root\0\nA\0a.txt\0";

    /// `--no-follow` of `a.txt` (no `--no-merges`): the conflict-resolving merge has no entries.
    const NO_FOLLOW: &[u8] = b"\x1e3616b768b52d451f451e55b205858df118bbb9e6\x001d5bb77a9a86f2eee6a126e30f95e66e93d4aaaf\0Ann Author\0ann@example.com\x001700054000\0rename a -> b with edit\0\nD\0a.txt\0\
    \x1e7cb9a69bd02197a39f4c4bdd2161e61ac0fb1028\x00787bdad61eae93de67da95463aa1ce33451de0f9 4a9de4fd3a251fc95a29d57ebd7985bc12427ff9\0Ann Author\0ann@example.com\x001700046800\0merge evil (resolved)\0\
    \x1e787bdad61eae93de67da95463aa1ce33451de0f9\x009865c96114e3a56b2e9f709bca3774a613857889\0Ann Author\0ann@example.com\x001700043200\0main side edit\0\nM\0a.txt\0\
    \x1e4a9de4fd3a251fc95a29d57ebd7985bc12427ff9\x009865c96114e3a56b2e9f709bca3774a613857889\0Ann Author\0ann@example.com\x001700039600\0evil side edit\0\nM\0a.txt\0\
    \x1e2760f540e35e521675c80c8c4eda70787d7fef37\x0054c1e9bb3f5149c2de7e7e7f97709dcfc162ec23\0Ann Author\0ann@example.com\x001700028800\0feat: edit a line 10\0\nM\0a.txt\0\
    \x1e54c1e9bb3f5149c2de7e7e7f97709dcfc162ec23\x0000e8e9bae805d3a52163cef76426f280e6cc7511\0Ann Author\0ann@example.com\x001700025200\0edit a line 1\0\nM\0a.txt\0\
    \x1e00e8e9bae805d3a52163cef76426f280e6cc7511\0\0Ann Author\0ann@example.com\x001700021600\0root\0\nA\0a.txt\0";

    /// A file deleted at HEAD~n, listed from HEAD.
    const DELETED: &[u8] = b"\x1e1d5bb77a9a86f2eee6a126e30f95e66e93d4aaaf\x007cb9a69bd02197a39f4c4bdd2161e61ac0fb1028\0Ann Author\0ann@example.com\x001700050400\0delete gone\0\nD\0gone.txt\0\
    \x1e54c1e9bb3f5149c2de7e7e7f97709dcfc162ec23\x0000e8e9bae805d3a52163cef76426f280e6cc7511\0Ann Author\0ann@example.com\x001700025200\0edit a line 1\0\nM\0gone.txt\0\
    \x1e00e8e9bae805d3a52163cef76426f280e6cc7511\0\0Ann Author\0ann@example.com\x001700021600\0root\0\nA\0gone.txt\0";

    /// `--follow` of `copy.txt`: `--follow` reports the copy (C100) even with plain `-M`; the root
    /// commit's subject contains an RS byte.
    const COPY: &[u8] = b"\x1eeac0210998a8654c3809cc12b83942c2dc01d9c6\0fadbba61c0668ce255f37bd16fb29cc075675bca\0Ann Author\0ann@example.com\x001700010800\0copy f to copy.txt\0\nC100\0f.txt\0copy.txt\0\
    \x1efadbba61c0668ce255f37bd16fb29cc075675bca\09b9abb8753c7c9dbffb0837039e765e3ce3ad3be\0Ann Author\0ann@example.com\x001700007200\0edit all\0\nM\0f.txt\0\
    \x1e9b9abb8753c7c9dbffb0837039e765e3ce3ad3be\0\0Ann Author\0ann@example.com\x001700003600\0root with \x1e RS in subject\0\nA\0f.txt\0";

    /// `--follow` of a file whose old and new names contain LF.
    const NEWLINE: &[u8] = b"\x1e497c102ba983360f635dd8d7eb5f1d6af74b90b1\0eac0210998a8654c3809cc12b83942c2dc01d9c6\0Ann Author\0ann@example.com\x001700014400\0rename nl\0\nR100\0nl\nname.txt\0nl2\nname.txt\0\
    \x1efadbba61c0668ce255f37bd16fb29cc075675bca\09b9abb8753c7c9dbffb0837039e765e3ce3ad3be\0Ann Author\0ann@example.com\x001700007200\0edit all\0\nM\0nl\nname.txt\0\
    \x1e9b9abb8753c7c9dbffb0837039e765e3ce3ad3be\0\0Ann Author\0ann@example.com\x001700003600\0root with \x1e RS in subject\0\nA\0nl\nname.txt\0";

    fn rows(v: &[HistoryEntry]) -> Vec<(&str, &str, &str, Option<&str>)> {
        v.iter().map(|e| (&e.oid[..7], e.status.as_str(), e.path.as_str(), e.old_path.as_deref())).collect()
    }

    #[test]
    fn follow_across_rename() {
        let h = parse_log(FOLLOW, "b 한.txt");
        assert_eq!(
            rows(&h),
            vec![
                ("c84d9ef", "M", "b 한.txt", None),
                ("e5bc7a2", "M", "b 한.txt", None),
                ("3616b76", "R", "b 한.txt", Some("a.txt")),
                ("787bdad", "M", "a.txt", None),
                ("4a9de4f", "M", "a.txt", None),
                ("2760f54", "M", "a.txt", None),
                ("54c1e9b", "M", "a.txt", None),
                ("00e8e9b", "A", "a.txt", None),
            ]
        );
        let first = &h[0];
        assert_eq!(first.oid, "c84d9efd1c8b340b7fdb9e4eb8036e5ef7a84099");
        assert_eq!(first.parents, vec!["e5bc7a2801fb6586d26817f6a495e43e6612ce4d"]);
        assert_eq!((first.author.as_str(), first.email.as_str()), ("Ann Author", "ann@example.com"));
        assert_eq!((first.time, first.subject.as_str()), (1700061200, "make b executable"));
        assert!(h[7].parents.is_empty());
        assert_eq!(h[2].subject, "rename a -> b with edit");
    }

    #[test]
    fn merges_without_entries_are_skipped() {
        let h = parse_log(NO_FOLLOW, "a.txt");
        assert_eq!(
            rows(&h),
            vec![
                ("3616b76", "D", "a.txt", None),
                ("787bdad", "M", "a.txt", None),
                ("4a9de4f", "M", "a.txt", None),
                ("2760f54", "M", "a.txt", None),
                ("54c1e9b", "M", "a.txt", None),
                ("00e8e9b", "A", "a.txt", None),
            ]
        );
    }

    #[test]
    fn deleted_file_from_head() {
        let h = parse_log(DELETED, "gone.txt");
        assert_eq!(
            rows(&h),
            vec![("1d5bb77", "D", "gone.txt", None), ("54c1e9b", "M", "gone.txt", None), ("00e8e9b", "A", "gone.txt", None)]
        );
        assert_eq!(h[0].parents, vec!["7cb9a69bd02197a39f4c4bdd2161e61ac0fb1028"]);
    }

    #[test]
    fn copy_rs_in_subject_and_newlines_in_paths() {
        let h = parse_log(COPY, "copy.txt");
        assert_eq!(
            rows(&h),
            vec![("eac0210", "C", "copy.txt", Some("f.txt")), ("fadbba6", "M", "f.txt", None), ("9b9abb8", "A", "f.txt", None)]
        );
        assert_eq!((h[2].subject.as_str(), h[2].time), ("root with \u{1e} RS in subject", 1700003600));
        assert!(h[2].parents.is_empty());
        let h = parse_log(NEWLINE, "nl2\nname.txt");
        assert_eq!(
            rows(&h),
            vec![
                ("497c102", "R", "nl2\nname.txt", Some("nl\nname.txt")),
                ("fadbba6", "M", "nl\nname.txt", None),
                ("9b9abb8", "A", "nl\nname.txt", None),
            ]
        );
    }

    #[test]
    fn picks_the_followed_entry_and_tolerates_odd_bytes() {
        let buf = b"\x1e1111111\0\0A\0a@x\x001\0two files\0\nM\0other\x1e.txt\0C075\0src.txt\0f.txt\0\
            \x1e2222222\0\0A\0a@x\x002\0older\0\nM\0src.txt\0M\0f.txt\0\
            \x1e3333333\0\0A\0a@x\0";
        let h = parse_log(buf, "f.txt");
        assert_eq!(rows(&h), vec![("1111111", "C", "f.txt", Some("src.txt")), ("2222222", "M", "src.txt", None)]);
        assert_eq!(h[1].time, 2);
        assert!(parse_log(b"", "f.txt").is_empty());
    }

    fn registered(repo: &Path) -> usize {
        RUNNING.lock().unwrap_or_else(|e| e.into_inner()).iter().filter(|(r, _)| r.as_path() == repo).count()
    }

    #[test]
    fn a_new_history_cancels_the_running_one_of_its_repo() {
        // Paths no other test registers (the registry is process-wide).
        let (a, b) = (Path::new("/nonexistent/history-run-a"), Path::new("/nonexistent/history-run-b"));
        let first = Run::begin(a);
        let other = Run::begin(b);
        assert!(!first.cancelled());
        let second = Run::begin(a);
        assert!(first.cancelled() && !second.cancelled());
        // Other repositories keep their run.
        assert!(!other.cancelled());
        // A superseded run that finishes leaves its successor registered...
        drop(first);
        assert_eq!(registered(a), 1);
        let third = Run::begin(a);
        assert!(second.cancelled() && !third.cancelled());
        // ...and the newest run unregisters when it finishes.
        drop(second);
        drop(third);
        drop(other);
        assert_eq!((registered(a), registered(b)), (0, 0));
    }

    #[test]
    fn closing_a_repo_cancels_only_its_history() {
        let (a, b) = (Path::new("/nonexistent/history-close-a"), Path::new("/nonexistent/history-close-b"));
        let run = Run::begin(a);
        let other = Run::begin(b);
        cancel(a);
        assert!(run.cancelled() && !other.cancelled());
        assert_eq!((registered(a), registered(b)), (0, 1));
        // Nothing left to cancel; the run ending later changes nothing, and a new run starts clean.
        cancel(a);
        drop(run);
        let next = Run::begin(a);
        assert!(!next.cancelled());
        drop(next);
        drop(other);
        assert_eq!((registered(a), registered(b)), (0, 0));
    }
}
