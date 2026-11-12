//! Reading commits for the graph via `git log`.

use crate::error::AppResult;
use crate::git::cmd::Git;
use crate::model::GraphOrder;
use memchr::memchr;
use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

/// Six fields per commit; `-z` turns the record terminator into NUL too, so the output is a flat
/// NUL-separated field list.
const FORMAT: &str = "--format=%H%x00%P%x00%aN%x00%aE%x00%at%x00%s";

/// Fields per record in `FORMAT`.
const FIELDS: usize = 6;

/// Oid and body of a commit (`read_merge_bodies`); `-z` ends each with a NUL, the body keeps its own
/// newlines (git prints no NUL inside a message).
const BODY_FORMAT: &str = "--format=%H%x00%b";

/// One commit as read from `git log` (subject only; full message is fetched on demand).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RawCommit {
    pub oid: String,
    pub parents: Vec<String>,
    pub author: String,
    pub email: String,
    /// Author time, unix seconds.
    pub time: i64,
    pub subject: String,
}

/// Zero-copy view of one record of `git log` output.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LogRecord<'a> {
    pub oid: &'a [u8],
    /// Space-separated parent oids; empty for root commits and shallow boundaries.
    pub parents: &'a [u8],
    pub author: &'a [u8],
    pub email: &'a [u8],
    /// Author time, unix seconds (0 when unparsable).
    pub time: i64,
    pub subject: &'a [u8],
}

impl<'a> LogRecord<'a> {
    /// Parent oids in order.
    pub fn parent_oids(&self) -> impl Iterator<Item = &'a [u8]> {
        self.parents.split(|&b| b == b' ').filter(|p| !p.is_empty())
    }

    /// Owned copy with every field decoded as lossy UTF-8.
    pub fn to_raw(&self) -> RawCommit {
        RawCommit {
            oid: lossy(self.oid),
            parents: self.parent_oids().map(lossy).collect(),
            author: lossy(self.author),
            email: lossy(self.email),
            time: self.time,
            subject: lossy(self.subject),
        }
    }
}

/// Iterator over the records of a log buffer, see [`records`].
pub struct Records<'a> {
    buf: &'a [u8],
    pos: usize,
}

/// Records of `git log -z <FORMAT>` output, one `memchr` pass over the buffer. The last record may be
/// NUL-terminated (`--format`) or not (`--pretty=format:`); an incomplete trailing record and records
/// without an oid are skipped.
pub fn records(buf: &[u8]) -> Records<'_> {
    Records { buf, pos: 0 }
}

impl<'a> Iterator for Records<'a> {
    type Item = LogRecord<'a>;

    fn next(&mut self) -> Option<LogRecord<'a>> {
        let buf = self.buf;
        loop {
            if self.pos >= buf.len() {
                return None;
            }
            let mut f: [&'a [u8]; FIELDS] = [&[]; FIELDS];
            for (k, field) in f.iter_mut().enumerate() {
                let rest = &buf[self.pos..];
                match memchr(0, rest) {
                    Some(n) => {
                        *field = &rest[..n];
                        self.pos += n + 1;
                    }
                    // Unterminated tail: only valid as the subject of the final record.
                    None if k == FIELDS - 1 => {
                        *field = rest;
                        self.pos = buf.len();
                    }
                    None => {
                        self.pos = buf.len();
                        return None;
                    }
                }
            }
            if f[0].is_empty() {
                continue;
            }
            return Some(LogRecord {
                oid: f[0],
                parents: f[1],
                author: f[2],
                email: f[3],
                time: parse_time(f[4]),
                subject: f[5],
            });
        }
    }
}

/// Number of records in a log buffer, for pre-sizing (counts NULs, one SIMD pass).
pub fn record_count_hint(buf: &[u8]) -> usize {
    memchr::memchr_iter(0, buf).count() / FIELDS + 1
}

/// `git log` order flag; both orders put every child before its parents.
pub fn order_flag(order: GraphOrder) -> &'static str {
    match order {
        GraphOrder::Date => "--date-order",
        GraphOrder::Topo => "--topo-order",
    }
}

/// `--stdin` input: one revision per line. Lines that git would parse as options (or that would break
/// the line format) are dropped; revisions are oids or full ref names, optionally prefixed with `^`.
pub fn rev_input(revs: &[String]) -> Vec<u8> {
    let mut out = Vec::with_capacity(revs.iter().map(|r| r.len() + 1).sum());
    for r in revs {
        if r.is_empty() || r.starts_with('-') || r.contains(['\n', '\r', '\0']) {
            continue;
        }
        out.extend_from_slice(r.as_bytes());
        out.push(b'\n');
    }
    out
}

/// Raw `git log` output (see [`records`]) for the history reachable from `tips`, at most `limit`
/// commits (0 = unlimited). Empty without running git when there are no tips: `log --stdin` with no
/// input would fall back to HEAD.
/// `first_parent`: follow first parents only (the simple view's line of one branch). `%P` still lists
/// every parent of a merge: the caller keeps the first.
/// `cancel`: a newer graph load sets it to kill this walk.
pub fn read_raw(
    cwd: &Path,
    tips: &[String],
    order: GraphOrder,
    limit: u32,
    first_parent: bool,
    cancel: Option<Arc<AtomicBool>>,
) -> AppResult<Vec<u8>> {
    let input = rev_input(tips);
    if input.is_empty() {
        return Ok(Vec::new());
    }
    let mut git = Git::new(cwd).args(["log", "-z", "--no-decorate", order_flag(order), "--ignore-missing", FORMAT]);
    if first_parent {
        git = git.arg("--first-parent");
    }
    if limit > 0 {
        git = git.arg(format!("-n{limit}"));
    }
    if let Some(flag) = cancel {
        git = git.cancel_on(flag);
    }
    git.args(["--stdin", "--"]).stdin(input).run()
}

/// Raw `git log --first-parent --merges` output over `tips` (see [`merge_bodies`]): oid and body of
/// each merge commit on the first-parent walk `read_raw(.., first_parent: true, ..)` reads, for the
/// pull request titles in them (`pr_title`). The same order and limit: the merges among that walk's
/// commits are the first ones here. Only merges: bodies are the bulk of a message, and the other
/// commits carry no title of a pull request. Empty without tips, like `read_raw`.
pub fn read_merge_bodies(
    cwd: &Path,
    tips: &[String],
    order: GraphOrder,
    limit: u32,
    cancel: Option<Arc<AtomicBool>>,
) -> AppResult<Vec<u8>> {
    let input = rev_input(tips);
    if input.is_empty() {
        return Ok(Vec::new());
    }
    let mut git = Git::new(cwd).args([
        "log",
        "-z",
        "--no-decorate",
        order_flag(order),
        "--ignore-missing",
        "--first-parent",
        "--merges",
        BODY_FORMAT,
    ]);
    if limit > 0 {
        git = git.arg(format!("-n{limit}"));
    }
    if let Some(flag) = cancel {
        git = git.cancel_on(flag);
    }
    git.args(["--stdin", "--"]).stdin(input).run()
}

/// (oid, body) pairs of [`read_merge_bodies`] output: NUL-separated fields, an oid then its body. A
/// field where an oid belongs that is none (were a NUL ever inside a message, the pairs would shift)
/// is skipped, so the pairs line up again at the next oid; an oid without a body ends the list.
pub fn merge_bodies(buf: &[u8]) -> Vec<(&[u8], &[u8])> {
    let mut out = Vec::new();
    let mut fields = buf.split(|&b| b == 0);
    while let Some(f) = fields.next() {
        if !is_oid(f) {
            continue;
        }
        match fields.next() {
            Some(body) => out.push((f, body)),
            None => break,
        }
    }
    out
}

/// A full hex oid as `%H` prints it: 40 (SHA-1) or 64 (SHA-256) lowercase hex digits.
fn is_oid(b: &[u8]) -> bool {
    (b.len() == 40 || b.len() == 64) && b.iter().all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(c))
}

/// GitLab's last line of a merge request's merge commit.
const SEE_MERGE_REQUEST: &str = "See merge request ";

/// The title of the pull request a merge commit merged: the first non-blank line of its body,
/// trimmed, when its subject is a hosting service's merge line —
/// - GitHub: `Merge pull request #12 from owner/branch`;
/// - Bitbucket: `Merged in branch (pull request #12)`;
/// - GitLab: `Merge branch 'x' into 'main'`, only with a `See merge request …` line in the body:
///   `git merge` itself writes such a subject (into a branch of another name), and its body is no
///   title (old git listed `Conflicts:` there).
///
/// None for any other commit, and when GitLab's `See merge request` line is all the body says.
pub fn pr_title<'a>(subject: &str, body: &'a str) -> Option<&'a str> {
    let gitlab = is_gitlab_merge(subject);
    if !gitlab && !is_github_merge(subject) && !is_bitbucket_merge(subject) {
        return None;
    }
    if gitlab && !body.lines().any(|l| l.trim_start().starts_with(SEE_MERGE_REQUEST)) {
        return None;
    }
    let line = body.lines().map(str::trim).find(|l| !l.is_empty())?;
    (!line.starts_with(SEE_MERGE_REQUEST)).then_some(line)
}

/// `Merge pull request #<digits> from …` (GitHub; Bitbucket Server's `… #12 in PROJ/repo from …` keeps
/// a list of commits in its body, no title).
fn is_github_merge(subject: &str) -> bool {
    subject.strip_prefix("Merge pull request #").is_some_and(|rest| {
        let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
        digits > 0 && rest[digits..].starts_with(" from ")
    })
}

/// `Merged in <branch> (pull request #<digits>)` (Bitbucket Cloud).
fn is_bitbucket_merge(subject: &str) -> bool {
    const PR: &str = " (pull request #";
    let Some(inner) = subject.strip_prefix("Merged in ").and_then(|r| r.strip_suffix(')')) else {
        return false;
    };
    inner.rfind(PR).is_some_and(|at| {
        let number = &inner[at + PR.len()..];
        !number.is_empty() && number.bytes().all(|b| b.is_ascii_digit())
    })
}

/// `Merge branch '<source>' into '<target>'` (GitLab quotes the target too; `git merge` does not).
fn is_gitlab_merge(subject: &str) -> bool {
    subject.strip_prefix("Merge branch '").is_some_and(|rest| rest.ends_with('\'') && rest.contains("' into '"))
}

/// Walk history from `tips` (oids or full ref names, fed via `--stdin`) in the given order.
/// `limit` 0 = unlimited. Children always precede parents in the result.
pub fn read_commits(cwd: &Path, tips: &[String], order: GraphOrder, limit: u32) -> AppResult<Vec<RawCommit>> {
    Ok(parse_log(&read_raw(cwd, tips, order, limit, false, None)?))
}

/// Exactly the given commits (no history walk), in input order; unknown oids are skipped.
pub fn read_exact(cwd: &Path, oids: &[String]) -> AppResult<Vec<RawCommit>> {
    let input = rev_input(oids);
    if input.is_empty() {
        return Ok(Vec::new());
    }
    let out = Git::new(cwd)
        .args(["log", "-z", "--no-decorate", "--no-walk=unsorted", "--ignore-missing", FORMAT, "--stdin", "--"])
        .stdin(input)
        .run()?;
    Ok(parse_log(&out))
}

/// Parse the raw output produced by the format used in `read_commits`.
pub fn parse_log(buf: &[u8]) -> Vec<RawCommit> {
    let mut out = Vec::with_capacity(record_count_hint(buf));
    out.extend(records(buf).map(|r| r.to_raw()));
    out
}

fn parse_time(b: &[u8]) -> i64 {
    std::str::from_utf8(b).ok().and_then(|s| s.trim().parse().ok()).unwrap_or(0)
}

fn lossy(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `git log -z --date-order --format=<FORMAT>` on a repo with a merge, an empty-message commit, a
    /// Korean/accented subject and a root commit (git 2.48).
    const REAL: &str = concat!(
        "6bd3ead629c2c36c7e2c5053eb5fff91cb360277\x005b8a6a37690d92167f8b98a729ca6a407b4f96ae 8c6d9f723cd02464166a3cf0dfa3b7220ab1eca1\x00Ann Author\x00ann@example.com\x001700000400\x00Merge branch 'side'\x00",
        "8c6d9f723cd02464166a3cf0dfa3b7220ab1eca1\x0077fa06895f705c2cd22d3f61383adcb6de0e2490\x00Ann Author\x00ann@example.com\x001700000300\x00side work\x00",
        "5b8a6a37690d92167f8b98a729ca6a407b4f96ae\x0077fa06895f705c2cd22d3f61383adcb6de0e2490\x00Ann Author\x00ann@example.com\x001700000200\x00\x00",
        "77fa06895f705c2cd22d3f61383adcb6de0e2490\x0063cf36a5371e38ea1c9fade3d1757921f2c6b713\x00Ann Author\x00ann@example.com\x001700000100\x00한글 커밋 메시지 ünïcödé\x00",
        "63cf36a5371e38ea1c9fade3d1757921f2c6b713\x00\x00Ann Author\x00ann@example.com\x001700000000\x00root commit\x00",
    );

    #[test]
    fn parses_real_output() {
        let v = parse_log(REAL.as_bytes());
        assert_eq!(v.len(), 5);
        assert_eq!(v[0].oid, "6bd3ead629c2c36c7e2c5053eb5fff91cb360277");
        assert_eq!(
            v[0].parents,
            ["5b8a6a37690d92167f8b98a729ca6a407b4f96ae", "8c6d9f723cd02464166a3cf0dfa3b7220ab1eca1"]
        );
        assert_eq!(v[0].author, "Ann Author");
        assert_eq!(v[0].email, "ann@example.com");
        assert_eq!(v[0].time, 1700000400);
        assert_eq!(v[0].subject, "Merge branch 'side'");
        assert_eq!(v[2].subject, "");
        assert_eq!(v[2].parents, ["77fa06895f705c2cd22d3f61383adcb6de0e2490"]);
        assert_eq!(v[3].subject, "한글 커밋 메시지 ünïcödé");
        assert_eq!(v[4].oid, "63cf36a5371e38ea1c9fade3d1757921f2c6b713");
        assert!(v[4].parents.is_empty());
        assert_eq!(v[4].subject, "root commit");
        assert_eq!(v[4].time, 1700000000);
    }

    #[test]
    fn tolerates_unterminated_last_record() {
        let full = parse_log(REAL.as_bytes());
        let cut = &REAL.as_bytes()[..REAL.len() - 1];
        assert_eq!(parse_log(cut), full);
        // Unterminated last record with an empty subject: the record still ends after five NULs.
        let empty_last = b"aaaa\x00\x00N\x00e@x\x001\x00";
        let v = parse_log(empty_last);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].subject, "");
        assert_eq!(v[0].time, 1);
    }

    #[test]
    fn drops_incomplete_and_empty_records() {
        assert!(parse_log(b"").is_empty());
        assert!(parse_log(b"abcd\x00ef").is_empty());
        let first_end = REAL.match_indices('\0').nth(FIELDS - 1).unwrap().0 + 1;
        let mut buf = REAL.as_bytes()[..first_end].to_vec();
        buf.extend_from_slice(b"8c6d9f723cd02464166a3cf0dfa3b7220ab1eca1\x00p\x00");
        let v = parse_log(&buf);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].subject, "Merge branch 'side'");
        assert!(parse_log(b"\x00\x00\x00\x00\x00\x00").is_empty());
    }

    #[test]
    fn decodes_invalid_utf8_lossily() {
        let v = parse_log(b"abcd\x00\x00Jos\xe9\x00j@x\x00x\x00caf\xe9\x00");
        assert_eq!(v[0].author, "Jos\u{fffd}");
        assert_eq!(v[0].subject, "caf\u{fffd}");
        assert_eq!(v[0].time, 0);
    }

    #[test]
    fn record_view_matches_owned() {
        let r = records(REAL.as_bytes()).next().unwrap();
        assert_eq!(r.parent_oids().count(), 2);
        assert_eq!(r.to_raw(), parse_log(REAL.as_bytes())[0]);
        assert_eq!(record_count_hint(REAL.as_bytes()), 6);
    }

    #[test]
    fn rev_input_drops_option_like_lines() {
        let revs: Vec<String> = ["abc", "", "--all", "^def", "a\nb"].iter().map(|s| s.to_string()).collect();
        assert_eq!(rev_input(&revs), b"abc\n^def\n");
        assert!(rev_input(&[]).is_empty());
    }

    #[test]
    fn no_tips_reads_nothing_without_git() {
        // `log --stdin` with empty input would walk HEAD; the directory doesn't exist, so running git fails.
        let none = Path::new("/nonexistent-git-viewer-dir");
        assert!(read_commits(none, &[], GraphOrder::Date, 0).unwrap().is_empty());
        let dropped = vec![String::new(), "--all".to_string()];
        assert!(read_raw(none, &dropped, GraphOrder::Topo, 10, true, None).unwrap().is_empty());
        assert!(read_merge_bodies(none, &dropped, GraphOrder::Date, 0, None).unwrap().is_empty());
        assert!(read_exact(none, &dropped).unwrap().is_empty());
        assert!(read_commits(none, &["abcd".to_string()], GraphOrder::Date, 0).is_err());
    }

    /// `git log -z --first-parent --merges --format=%H%x00%b` on a repo with a GitLab merge request
    /// merge, a GitHub-style merge, a plain `git merge` without a body and one with old git's
    /// conflict list (git 2.48).
    const BODIES: &[u8] = b"6a2ddfaba2a852861df5fc1cc321274edfb39039\0\0\
        cab786bdf867fb4de10672f221e94730da3b027a\0Gitlab title\n\nSee merge request grp/proj!7\n\0\
        9151dc91c08de7653f6fd67c0f1b5cec5608d025\0Add the feature\n\0\
        0f1e2d3c4b5a69788796a5b4c3d2e1f00f1e2d3c\0Conflicts:\n\tsrc/a.c\n\0";

    #[test]
    fn parses_merge_bodies() {
        let v = merge_bodies(BODIES);
        let oids: Vec<&[u8]> = v.iter().map(|p| p.0).collect();
        assert_eq!(
            oids,
            [
                &b"6a2ddfaba2a852861df5fc1cc321274edfb39039"[..],
                b"cab786bdf867fb4de10672f221e94730da3b027a",
                b"9151dc91c08de7653f6fd67c0f1b5cec5608d025",
                b"0f1e2d3c4b5a69788796a5b4c3d2e1f00f1e2d3c",
            ]
        );
        assert_eq!(v[0].1, b"");
        assert_eq!(v[1].1, b"Gitlab title\n\nSee merge request grp/proj!7\n");
        assert_eq!(v[3].1, b"Conflicts:\n\tsrc/a.c\n");
        // Unterminated last body; an oid without a body; a stray field where an oid belongs.
        assert_eq!(merge_bodies(&BODIES[..BODIES.len() - 1]).len(), 4);
        let cut = b"9151dc91c08de7653f6fd67c0f1b5cec5608d025";
        assert!(merge_bodies(cut).is_empty());
        let shifted = b"not an oid\09151dc91c08de7653f6fd67c0f1b5cec5608d025\0Title\n\0";
        assert_eq!(merge_bodies(shifted), [(&cut[..], &b"Title\n"[..])]);
        let sha256 = [b'a'; 64];
        let mut buf = sha256.to_vec();
        buf.extend_from_slice(b"\0T\0");
        assert_eq!(merge_bodies(&buf), [(&sha256[..], &b"T"[..])]);
        assert!(merge_bodies(b"").is_empty());
        assert!(merge_bodies(b"ABCDEF0123ABCDEF0123ABCDEF0123ABCDEF0123\0upper case\0").is_empty());
    }

    #[test]
    fn pull_request_titles() {
        let gh = "Merge pull request #123 from acme/add-onboarding";
        assert_eq!(pr_title(gh, "Add onboarding screens\n"), Some("Add onboarding screens"));
        assert_eq!(pr_title(gh, "\n  Spaced title \r\n\nA description.\n"), Some("Spaced title"));
        assert_eq!(pr_title(gh, ""), None);
        assert_eq!(pr_title(gh, " \n\t\n"), None);
        assert_eq!(pr_title("Merge pull request #12 in PROJ/repo from x to main", "* commit 'abc'"), None);
        assert_eq!(pr_title("Merge pull request # from x", "T"), None);
        assert_eq!(pr_title("Merge pull request #12from x", "T"), None);

        let bb = "Merged in feature/login (pull request #42)";
        assert_eq!(pr_title(bb, "Login page\n\n* commit one\n\nApproved-by: A\n"), Some("Login page"));
        assert_eq!(pr_title("Merged in feature/login (pull request #)", "T"), None);
        assert_eq!(pr_title("Merged in feature/login", "T"), None);

        let gl = "Merge branch 'feature' into 'main'";
        let body = "Add the cart\n\nCloses #4\n\nSee merge request group/shop!12\n";
        assert_eq!(pr_title(gl, body), Some("Add the cart"));
        // GitLab without a title of its own: only its last line.
        assert_eq!(pr_title(gl, "See merge request group/shop!12\n"), None);
        // `git merge` writes such subjects too: without GitLab's last line it has no title.
        assert_eq!(pr_title(gl, "Conflicts:\n\tsrc/a.c\n"), None);
        assert_eq!(pr_title("Merge branch 'feature' into develop", body), None);
        assert_eq!(pr_title("Merge branch 'feature'", "Conflicts:\n\tsrc/a.c\n"), None);
        assert_eq!(pr_title("Merge remote-tracking branch 'origin/main'", "Title"), None);
        // Not a merge line: a squashed pull request already has its title as the subject.
        assert_eq!(pr_title("Add onboarding screens (#123)", "Body line"), None);
    }
}
